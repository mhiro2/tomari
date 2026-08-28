import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import type { ReactElement } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { SettingsProvider, useSettings } from '../lib/settings';
import type { AppSettings, Hotkey, ModifierRule } from '../lib/types';
import { KeyboardView } from './KeyboardView';

// Mock the Tauri command bridge so the real `api` wrappers run against it.
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
const { invoke } = await import('@tauri-apps/api/core');
const mockInvoke = vi.mocked(invoke);

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
  menuBarTidyEnabled: false,
  menuBarAutoCollapseSecs: 0,
};

const RULE: ModifierRule = {
  id: 'rule-1',
  label: 'Caps Lock',
  modifier: 'capsLock',
  side: 'either',
  remapTo: 'control',
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

const WINDOW_HOTKEY: Hotkey = {
  id: 'hk-window',
  label: 'Restore editor position',
  accelerator: 'Ctrl+Alt+Down',
  action: { type: 'recallWindowPlacement' },
  enabled: true,
};

// KeyboardView reads the master switch from the shared settings provider.
function renderView(ui: ReactElement) {
  return render(<SettingsProvider>{ui}</SettingsProvider>);
}

async function openShortcuts() {
  const tabs = await screen.findByRole('tablist');
  fireEvent.click(within(tabs).getByRole('tab', { name: 'Shortcuts' }));
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
      case 'delete_modifier_rule':
        return Promise.resolve({ applyWarnings: [] });
      case 'save_hotkey':
      case 'delete_hotkey':
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

// Reads the shared apply-warning state the rule mutations report into.
function ApplyWarningsProbe() {
  const { applyWarnings } = useSettings();
  return <span data-testid="apply-warnings">{applyWarnings.join(',')}</span>;
}

describe('KeyboardView', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockCommands();
  });

  it('presents modifier behavior as a four-column mapping and shows the Command-key map', async () => {
    renderView(<KeyboardView />);

    const table = await screen.findByRole('table');
    expect(within(table).getByRole('columnheader', { name: 'Key' })).toBeInTheDocument();
    expect(within(table).getByRole('columnheader', { name: 'Tap' })).toBeInTheDocument();
    expect(within(table).getByRole('columnheader', { name: 'Hold' })).toBeInTheDocument();
    expect(within(table).getByRole('columnheader', { name: 'On' })).toBeInTheDocument();
    expect(within(table).getByRole('rowheader', { name: /Caps Lock/ })).toBeInTheDocument();
    expect(within(table).getByText('Used as Control')).toBeInTheDocument();

    expect(screen.getByRole('heading', { name: 'Input switching' })).toBeInTheDocument();
    expect(screen.getByText('Left Command')).toBeInTheDocument();
    expect(screen.getByText('English')).toBeInTheDocument();
    expect(screen.getByText('Right Command')).toBeInTheDocument();
    expect(screen.getByText('Japanese')).toBeInTheDocument();
  });

  it('keeps the modifier map visible but disables its inputs when Keyboard is off', async () => {
    mockCommands({ get_settings: { ...SETTINGS, keyboardEnabled: false } });

    renderView(<KeyboardView />);

    const table = await screen.findByRole('table');
    expect(within(table).getByRole('rowheader', { name: /Caps Lock/ })).toBeInTheDocument();
    expect(await screen.findByLabelText('Tap action for Caps Lock')).toBeDisabled();
    expect(screen.getByLabelText('Enable Caps Lock')).toBeDisabled();
    expect(screen.getByLabelText('Enable Modifier keys and shortcuts')).not.toBeDisabled();
    expect(screen.getByLabelText('Enable Modifier keys and shortcuts')).toHaveAttribute(
      'aria-checked',
      'false',
    );
  });

  it('opens the shortcut builder only after the add-shortcut action', async () => {
    renderView(<KeyboardView />);
    await openShortcuts();

    expect(screen.queryByRole('textbox', { name: 'Shortcut label' })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Add Shortcut' }));
    expect(screen.getByRole('textbox', { name: 'Shortcut label' })).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Cancel adding shortcut' }));
    expect(screen.queryByRole('textbox', { name: 'Shortcut label' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Add Shortcut' })).toBeInTheDocument();
  });

  it('shows an error when the initial modifier rules and hotkeys load fails', async () => {
    mockCommands({
      list_modifier_rules: Object.assign(new Error('boom'), { code: 'unknown' }),
      list_hotkeys: Object.assign(new Error('kaboom'), { code: 'unknown' }),
    });

    renderView(<KeyboardView />);

    expect(await screen.findByText('boom')).toBeInTheDocument();
    await openShortcuts();
    expect(await screen.findByText('kaboom')).toBeInTheDocument();
  });

  it('does not double-save or revert to a stale value when the toggle is clicked while a save is in flight', async () => {
    // Hold `save_modifier_rule` open so the row stays in its "saving" state.
    let resolveSave: (() => void) | undefined;
    const outcome = { applyWarnings: [] };
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
          return new Promise<{ applyWarnings: string[] }>((resolve) => {
            resolveSave = () => resolve(outcome);
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

  it('reports a rule save whose Caps Lock remap did not follow into the shared apply warnings', async () => {
    let capsOk = false;
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case 'list_modifier_rules':
          return Promise.resolve([RULE]);
        case 'list_hotkeys':
          return Promise.resolve([]);
        case 'get_settings':
          return Promise.resolve(SETTINGS);
        case 'save_modifier_rule':
          return Promise.resolve({ applyWarnings: capsOk ? [] : ['capsLockRemap'] });
        default:
          return Promise.resolve(null);
      }
    });

    renderView(
      <>
        <KeyboardView />
        <ApplyWarningsProbe />
      </>,
    );

    const toggle = await screen.findByLabelText('Enable Caps Lock');
    fireEvent.click(toggle);
    // The rule saved (no error) but the mismatch is reported, not swallowed.
    await waitFor(() =>
      expect(screen.getByTestId('apply-warnings')).toHaveTextContent('capsLockRemap'),
    );

    // A later save that applies cleanly clears it.
    capsOk = true;
    await waitFor(() => expect(toggle).not.toBeDisabled());
    fireEvent.click(toggle);
    await waitFor(() => expect(screen.getByTestId('apply-warnings')).toHaveTextContent(''));
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

  it('assigns Restore as a generic modifier tap action', async () => {
    renderView(<KeyboardView />);

    fireEvent.change(await screen.findByLabelText('Tap action for Caps Lock'), {
      target: { value: 'restoreWindow' },
    });

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('save_modifier_rule', {
        rule: expect.objectContaining({
          id: RULE.id,
          tap: { type: 'recallWindowPlacement' },
        }),
      }),
    );
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
    await openShortcuts();
    const toggle = await screen.findByLabelText('Enable Toggle Tomari');
    fireEvent.click(toggle);
    expect(toggle).toBeDisabled();

    // A second click while the save is in flight must not fire another save.
    fireEvent.click(toggle);
    expect(mockInvoke.mock.calls.filter((c) => c[0] === 'save_hotkey')).toHaveLength(1);

    resolveToggleSave?.();
    await waitFor(() => expect(toggle).not.toBeDisabled());
    expect(toggle).toHaveAttribute('aria-checked', 'true');
  });

  it('requires a second click to delete a hotkey, and the first click can be backed out of', async () => {
    mockCommands();
    renderView(<KeyboardView />);
    await openShortcuts();

    const deleteButton = await screen.findByLabelText('Delete Toggle Tomari');
    fireEvent.click(deleteButton);

    // Armed, but not yet deleted.
    expect(mockInvoke.mock.calls.filter((c) => c[0] === 'delete_hotkey')).toHaveLength(0);
    const confirmButton = await screen.findByLabelText('Delete Toggle Tomari?');
    expect(confirmButton).toHaveTextContent('Delete?');

    // Escape backs out without deleting.
    fireEvent.keyDown(confirmButton, { key: 'Escape' });
    expect(await screen.findByLabelText('Delete Toggle Tomari')).toBeInTheDocument();
    expect(mockInvoke.mock.calls.filter((c) => c[0] === 'delete_hotkey')).toHaveLength(0);

    // Arm again, then confirm.
    fireEvent.click(await screen.findByLabelText('Delete Toggle Tomari'));
    fireEvent.click(await screen.findByLabelText('Delete Toggle Tomari?'));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('delete_hotkey', { id: HOTKEY.id });
    });
  });

  it('shows empty-state messages when there are no modifier rules or hotkeys', async () => {
    mockCommands({ list_modifier_rules: [], list_hotkeys: [] });

    renderView(<KeyboardView />);

    expect(await screen.findByText('No modifier keys to configure.')).toBeInTheDocument();
    await openShortcuts();
    expect(
      await screen.findByText('No global shortcuts yet. Use Add Shortcut to create one.'),
    ).toBeInTheDocument();
  });

  it('leaves window shortcuts to the contextual Windows section', async () => {
    mockCommands({ list_hotkeys: [HOTKEY, WINDOW_HOTKEY] });

    renderView(<KeyboardView />);
    await openShortcuts();

    expect(await screen.findByText('Toggle panel')).toBeInTheDocument();
    expect(screen.queryByText('Restore editor position')).not.toBeInTheDocument();
  });
});
