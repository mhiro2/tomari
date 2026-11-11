import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { ReactElement } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { resetUpdateStateForTest } from '../lib/appUpdate';
import { SettingsProvider } from '../lib/settings';
import type { AppSettings } from '../lib/types';
import { GeneralView } from './GeneralView';

// Mock the Tauri command bridge so the real `api` wrappers run against it.
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
const { invoke } = await import('@tauri-apps/api/core');
const mockInvoke = vi.mocked(invoke);

// GeneralView reads the app version directly via `@tauri-apps/api/app`, which
// has no jsdom-compatible implementation (like the event API mocked globally
// in vitest.setup.ts).
vi.mock('@tauri-apps/api/app', () => ({ getVersion: vi.fn(() => Promise.resolve('1.2.3')) }));

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

// GeneralView reads settings (master switches live elsewhere) from the shared provider.
function renderView(ui: ReactElement) {
  return render(<SettingsProvider>{ui}</SettingsProvider>);
}

// A rejection value is wrapped in a thunk so the promise (and its rejection)
// is only created when the command actually runs, not when the test sets up
// the mock — otherwise it becomes an unhandled rejection before anything
// under test has a chance to attach a .catch.
function mockCommands(overrides: Record<string, unknown> = {}) {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd in overrides) {
      const value = overrides[cmd];
      const resolved = typeof value === 'function' ? value() : value;
      return resolved instanceof Promise ? resolved : Promise.resolve(resolved);
    }
    switch (cmd) {
      case 'get_settings':
        return Promise.resolve(SETTINGS);
      case 'save_settings':
        return Promise.resolve({ applyWarnings: [] });
      case 'get_configuration_backup_status':
        return Promise.resolve({ canRestore: false, lastApplied: null });
      default:
        return Promise.resolve(null);
    }
  });
}

function checkCalls() {
  return mockInvoke.mock.calls.filter(([cmd]) => cmd === 'check_for_update');
}

describe('GeneralView', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockCommands();
    resetUpdateStateForTest();
  });

  it('checks for an update and shows "up to date" when none is found', async () => {
    mockCommands({ check_for_update: null });
    renderView(<GeneralView />);

    const button = await screen.findByText('Check for Updates');
    fireEvent.click(button);

    // While the check is in flight, the button reflects the checking phase.
    expect(await screen.findByText('Checking…')).toBeInTheDocument();

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('check_for_update');
    });
    expect(await screen.findByText('Tomari is up to date.')).toBeInTheDocument();
    expect(await screen.findByText('Check for Updates')).toBeInTheDocument();
  });

  it('guards against overlapping checks: a rapid re-click does not double-invoke', async () => {
    let resolveCheck: ((value: null) => void) | undefined;
    mockCommands({
      check_for_update: new Promise((resolve) => {
        resolveCheck = resolve;
      }),
    });
    renderView(<GeneralView />);

    const button = await screen.findByText('Check for Updates');
    fireEvent.click(button);
    await screen.findByText('Checking…');

    // The button is disabled while checking, but exercise the guard itself
    // (the shared update state) by invoking the handler again directly.
    fireEvent.click(screen.getByText('Checking…'));
    fireEvent.click(screen.getByText('Checking…'));

    expect(checkCalls()).toHaveLength(1);

    resolveCheck?.(null);
    await waitFor(() => expect(screen.getByText('Check for Updates')).toBeEnabled());
    expect(checkCalls()).toHaveLength(1);
  });

  it('shows an install button when an update is available, then installs it', async () => {
    mockCommands({
      check_for_update: { version: '9.9.9', notes: 'Bug fixes' },
      install_update: new Promise(() => {}), // never resolves, like the real relaunch path
    });
    renderView(<GeneralView />);

    fireEvent.click(await screen.findByText('Check for Updates'));

    expect(await screen.findByText('Version 9.9.9 is available. Bug fixes')).toBeInTheDocument();
    const installButton = await screen.findByText('Install and Restart');
    expect(installButton).toBeInTheDocument();

    fireEvent.click(installButton);
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('install_update'));
    expect(await screen.findByText('Installing…')).toBeInTheDocument();
    expect(screen.getByText('Installing…')).toBeDisabled();
  });

  it('re-offers install and shows the error when install_update rejects', async () => {
    mockCommands({
      check_for_update: { version: '9.9.9', notes: null },
      install_update: () => Promise.reject({ message: 'network unreachable' }),
    });
    renderView(<GeneralView />);

    fireEvent.click(await screen.findByText('Check for Updates'));
    fireEvent.click(await screen.findByText('Install and Restart'));

    expect(
      await screen.findByText('Version 9.9.9 is available. Update failed: network unreachable'),
    ).toBeInTheDocument();
    // Install is offered again rather than leaving the panel stuck in "installing".
    expect(screen.getByText('Install and Restart')).toBeEnabled();
  });

  it('keeps the install in progress across a remount and a tray-requested check', async () => {
    mockCommands({
      check_for_update: { version: '9.9.9', notes: null },
      install_update: new Promise(() => {}),
    });
    const first = renderView(<GeneralView />);

    fireEvent.click(await screen.findByText('Check for Updates'));
    fireEvent.click(await screen.findByText('Install and Restart'));
    await screen.findByText('Installing…');

    // Navigating to another section and back remounts the view; the tray's
    // "Check for Updates" then asks the fresh mount to run a check.
    first.unmount();
    renderView(<GeneralView autoCheckUpdate />);

    expect(await screen.findByText('Installing…')).toBeDisabled();
    expect(screen.queryByText('Install and Restart')).not.toBeInTheDocument();
    fireEvent.click(screen.getByText('Installing…'));

    expect(checkCalls()).toHaveLength(1);
    expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === 'install_update')).toHaveLength(1);
  });

  it('keeps a running check across a remount instead of starting another', async () => {
    let resolveCheck: ((value: null) => void) | undefined;
    mockCommands({
      check_for_update: new Promise((resolve) => {
        resolveCheck = resolve;
      }),
    });
    const first = renderView(<GeneralView />);

    fireEvent.click(await screen.findByText('Check for Updates'));
    await screen.findByText('Checking…');

    first.unmount();
    renderView(<GeneralView autoCheckUpdate />);

    expect(await screen.findByText('Checking…')).toBeDisabled();
    expect(checkCalls()).toHaveLength(1);

    resolveCheck?.(null);
    expect(await screen.findByText('Tomari is up to date.')).toBeInTheDocument();
  });

  it('shows the error message when the update check fails', async () => {
    mockCommands({ check_for_update: () => Promise.reject({ message: 'offline' }) });
    renderView(<GeneralView />);

    fireEvent.click(await screen.findByText('Check for Updates'));

    expect(await screen.findByText('Could not check for updates: offline')).toBeInTheDocument();
    expect(await screen.findByText('Check for Updates')).toBeEnabled();
  });

  it('runs the update check automatically when asked to (tray "Check for Updates")', async () => {
    mockCommands({ check_for_update: null });
    const onAutoCheckHandled = vi.fn();
    renderView(<GeneralView autoCheckUpdate onAutoCheckHandled={onAutoCheckHandled} />);

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('check_for_update');
    });
    expect(await screen.findByText('Tomari is up to date.')).toBeInTheDocument();
    expect(onAutoCheckHandled).toHaveBeenCalled();
  });

  it('confirms before hiding the menu bar icon, and applies on confirmation', async () => {
    renderView(<GeneralView />);

    const toggle = await screen.findByRole('switch', { name: 'Show in menu bar' });
    expect(toggle).toHaveAttribute('aria-checked', 'true');

    fireEvent.click(toggle);

    // Turning off asks for confirmation instead of saving immediately.
    expect(await screen.findByText('Hide the menu bar icon?')).toBeInTheDocument();
    expect(mockInvoke).not.toHaveBeenCalledWith('save_settings', {
      settings: expect.objectContaining({ showInMenuBar: false }),
    });

    fireEvent.click(screen.getByText('Hide Icon'));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('save_settings', {
        settings: expect.objectContaining({ showInMenuBar: false }),
      });
    });
    expect(screen.queryByText('Hide the menu bar icon?')).not.toBeInTheDocument();
  });

  it('cancels the hide-menu-bar confirmation and leaves the setting untouched', async () => {
    renderView(<GeneralView />);

    const toggle = await screen.findByRole('switch', { name: 'Show in menu bar' });
    fireEvent.click(toggle);
    expect(await screen.findByText('Hide the menu bar icon?')).toBeInTheDocument();

    fireEvent.click(screen.getByText('Cancel'));

    expect(screen.queryByText('Hide the menu bar icon?')).not.toBeInTheDocument();
    expect(mockInvoke).not.toHaveBeenCalledWith('save_settings', {
      settings: expect.objectContaining({ showInMenuBar: false }),
    });
    // The toggle reverts to reflecting the unchanged (still-on) setting.
    expect(await screen.findByRole('switch', { name: 'Show in menu bar' })).toHaveAttribute(
      'aria-checked',
      'true',
    );
  });

  it('turning the menu bar icon back on needs no confirmation', async () => {
    mockCommands({ get_settings: { ...SETTINGS, showInMenuBar: false } });
    renderView(<GeneralView />);

    const toggle = await screen.findByRole('switch', { name: 'Show in menu bar' });
    expect(toggle).toHaveAttribute('aria-checked', 'false');

    fireEvent.click(toggle);

    expect(screen.queryByText('Hide the menu bar icon?')).not.toBeInTheDocument();
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('save_settings', {
        settings: expect.objectContaining({ showInMenuBar: true }),
      });
    });
  });
});
