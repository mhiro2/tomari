import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { ReactElement } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { SettingsProvider } from '../lib/settings';
import type { AppSettings, Hotkey, ModifierRule, PermissionsChanged } from '../lib/types';
import { KeyboardView } from './KeyboardView';

// Mock the Tauri command bridge so the real `api` wrappers run against it.
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
const { invoke } = await import('@tauri-apps/api/core');
const mockInvoke = vi.mocked(invoke);

// vitest.setup.ts stubs `listen` as a permanent no-op; capture the callback
// here so tests can drive the "tomari:permissions-changed" event directly.
const { listen } = await import('@tauri-apps/api/event');
const mockListen = vi.mocked(listen);

const SETTINGS: AppSettings = {
  launchAtLogin: false,
  language: 'system',
  keyboardEnabled: true,
  windowManagementEnabled: true,
  externalWindowActionsEnabled: true,
  commandImeSwitchEnabled: true,
  showInMenuBar: true,
  dragToSnapEnabled: false,
  dragToMoveEnabled: false,
};

const RULE: ModifierRule = {
  id: 'rule-1',
  label: 'Caps Lock',
  modifier: 'capsLock',
  side: 'either',
  hyper: false,
  tap: { type: 'noOp' },
  enabled: false,
};

const HOTKEY: Hotkey = {
  id: 'hk-1',
  label: 'Toggle panel',
  accelerator: 'Cmd+Shift+K',
  action: { type: 'togglePanel' },
  enabled: false,
};

// KeyboardView reads the master switch from the shared settings provider.
function renderView(ui: ReactElement) {
  return render(<SettingsProvider>{ui}</SettingsProvider>);
}

function mockCommands(overrides: Record<string, unknown> = {}) {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd in overrides) {
      const value = overrides[cmd];
      return value instanceof Error ? Promise.reject(value) : Promise.resolve(value);
    }
    switch (cmd) {
      case 'list_modifier_rules':
        return Promise.resolve([RULE]);
      case 'list_hotkeys':
        return Promise.resolve([HOTKEY]);
      case 'save_modifier_rule':
      case 'save_hotkey':
      case 'delete_hotkey':
      case 'delete_modifier_rule':
        return Promise.resolve(undefined);
      case 'get_settings':
        return Promise.resolve(SETTINGS);
      case 'input_monitoring_status':
        return Promise.resolve(true);
      default:
        return Promise.resolve(null);
    }
  });
}

describe('KeyboardView', () => {
  let permissionsChanged: ((payload: PermissionsChanged) => void) | undefined;

  beforeEach(() => {
    mockInvoke.mockReset();
    mockCommands();
    permissionsChanged = undefined;
    mockListen.mockReset();
    mockListen.mockImplementation((event, handler) => {
      if (event === 'tomari:permissions-changed') {
        permissionsChanged = (payload) =>
          (handler as (e: { event: string; id: number; payload: unknown }) => void)({
            event,
            id: 0,
            payload,
          });
      }
      return Promise.resolve(() => {});
    });
  });

  it('shows an error when the initial modifier rules and hotkeys load fails', async () => {
    mockCommands({
      list_modifier_rules: Object.assign(new Error('boom'), { code: 'unknown' }),
      list_hotkeys: Object.assign(new Error('kaboom'), { code: 'unknown' }),
    });

    renderView(<KeyboardView />);

    expect(await screen.findByText('boom')).toBeInTheDocument();
    expect(await screen.findByText('kaboom')).toBeInTheDocument();
  });

  it('does not double-save or revert to a stale value when the toggle is clicked while a save is in flight', async () => {
    // Hold `save_modifier_rule` open so the row stays in its "saving" state.
    let resolveSave: (() => void) | undefined;
    mockInvoke.mockImplementation((cmd: string, args?: unknown) => {
      switch (cmd) {
        case 'list_modifier_rules':
          return Promise.resolve([RULE]);
        case 'list_hotkeys':
          return Promise.resolve([]);
        case 'get_settings':
          return Promise.resolve(SETTINGS);
        case 'save_modifier_rule':
          expect((args as { rule: ModifierRule }).rule.enabled).toBe(true);
          return new Promise<void>((resolve) => {
            resolveSave = resolve;
          });
        default:
          return Promise.resolve(null);
      }
    });

    renderView(<KeyboardView />);

    const toggle = await screen.findByLabelText('Enable Caps Lock');
    fireEvent.click(toggle);

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('save_modifier_rule', {
        rule: expect.objectContaining({ enabled: true }),
      });
    });
    expect(toggle).toBeDisabled();

    // A second click while the save is in flight must not fire another save,
    // and must not revert the pending value once the first save lands.
    fireEvent.click(toggle);
    expect(mockInvoke.mock.calls.filter((c) => c[0] === 'save_modifier_rule')).toHaveLength(1);

    resolveSave?.();
    await waitFor(() => expect(toggle).not.toBeDisabled());
    expect(toggle).toHaveAttribute('aria-checked', 'true');
  });

  it('does not fire a second save_modifier_rule call from rapid repeated clicks', async () => {
    let resolveSave: (() => void) | undefined;
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case 'list_modifier_rules':
          return Promise.resolve([RULE]);
        case 'list_hotkeys':
          return Promise.resolve([]);
        case 'get_settings':
          return Promise.resolve(SETTINGS);
        case 'save_modifier_rule':
          return new Promise<void>((resolve) => {
            resolveSave = resolve;
          });
        default:
          return Promise.resolve(null);
      }
    });

    renderView(<KeyboardView />);
    const toggle = await screen.findByLabelText('Enable Caps Lock');

    fireEvent.click(toggle);
    fireEvent.click(toggle);
    fireEvent.click(toggle);

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('save_modifier_rule', expect.anything());
    });
    expect(mockInvoke.mock.calls.filter((c) => c[0] === 'save_modifier_rule')).toHaveLength(1);

    resolveSave?.();
  });

  it('disables a hotkey row while its save is in flight and re-enables it once persisted', async () => {
    let resolveToggleSave: (() => void) | undefined;
    mockInvoke.mockImplementation((cmd: string, args?: unknown) => {
      switch (cmd) {
        case 'list_modifier_rules':
          return Promise.resolve([]);
        case 'list_hotkeys':
          return Promise.resolve([HOTKEY]);
        case 'get_settings':
          return Promise.resolve(SETTINGS);
        case 'save_hotkey':
          expect((args as { hotkey: Hotkey }).hotkey.enabled).toBe(true);
          return new Promise<void>((resolve) => {
            resolveToggleSave = resolve;
          });
        default:
          return Promise.resolve(null);
      }
    });

    renderView(<KeyboardView />);
    const toggle = await screen.findByLabelText('Enable Toggle panel');
    fireEvent.click(toggle);
    expect(toggle).toBeDisabled();

    // A second click while the save is in flight must not fire another save.
    fireEvent.click(toggle);
    expect(mockInvoke.mock.calls.filter((c) => c[0] === 'save_hotkey')).toHaveLength(1);

    resolveToggleSave?.();
    await waitFor(() => expect(toggle).not.toBeDisabled());
    expect(toggle).toHaveAttribute('aria-checked', 'true');
  });

  it('shows the Input Monitoring banner when it is not granted', async () => {
    mockCommands({ input_monitoring_status: false });

    renderView(<KeyboardView />);

    expect(await screen.findByText('Input Monitoring access needed')).toBeInTheDocument();
  });

  it('does not show the Input Monitoring banner once it is granted', async () => {
    mockCommands({ input_monitoring_status: true });

    renderView(<KeyboardView />);

    // Wait for the initial load to settle before asserting the banner's absence.
    await screen.findByText('Caps Lock');
    expect(screen.queryByText('Input Monitoring access needed')).not.toBeInTheDocument();
  });

  it('hides the Input Monitoring banner once "tomari:permissions-changed" reports it granted', async () => {
    mockCommands({ input_monitoring_status: false });

    renderView(<KeyboardView />);
    expect(await screen.findByText('Input Monitoring access needed')).toBeInTheDocument();

    permissionsChanged?.({ accessibility: true, inputMonitoring: true });

    await waitFor(() => {
      expect(screen.queryByText('Input Monitoring access needed')).not.toBeInTheDocument();
    });
  });

  it('requests Input Monitoring access when the grant button is clicked', async () => {
    mockCommands({ input_monitoring_status: false, request_input_monitoring: true });

    renderView(<KeyboardView />);
    fireEvent.click(await screen.findByText('Grant Access'));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('request_input_monitoring');
    });
    await waitFor(() => {
      expect(screen.queryByText('Input Monitoring access needed')).not.toBeInTheDocument();
    });
  });

  it('requires a second click to delete a hotkey, and the first click can be backed out of', async () => {
    mockCommands();
    renderView(<KeyboardView />);

    const deleteButton = await screen.findByLabelText('Delete Toggle panel');
    fireEvent.click(deleteButton);

    // Armed, but not yet deleted.
    expect(mockInvoke.mock.calls.filter((c) => c[0] === 'delete_hotkey')).toHaveLength(0);
    const confirmButton = await screen.findByLabelText('Delete Toggle panel?');
    expect(confirmButton).toHaveTextContent('Delete?');

    // Escape backs out without deleting.
    fireEvent.keyDown(confirmButton, { key: 'Escape' });
    expect(await screen.findByLabelText('Delete Toggle panel')).toBeInTheDocument();
    expect(mockInvoke.mock.calls.filter((c) => c[0] === 'delete_hotkey')).toHaveLength(0);

    // Arm again, then confirm.
    fireEvent.click(await screen.findByLabelText('Delete Toggle panel'));
    fireEvent.click(await screen.findByLabelText('Delete Toggle panel?'));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('delete_hotkey', { id: HOTKEY.id });
    });
  });

  it('shows empty-state messages when there are no modifier rules or hotkeys', async () => {
    mockCommands({ list_modifier_rules: [], list_hotkeys: [] });

    renderView(<KeyboardView />);

    expect(await screen.findByText('No modifier keys to configure.')).toBeInTheDocument();
    expect(await screen.findByText('No global shortcuts yet — add one below.')).toBeInTheDocument();
  });
});
