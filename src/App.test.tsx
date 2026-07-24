import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { App } from './App';
import type { AppSettings, PermissionsChanged, SetupStatus } from './lib/types';

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
  language: 'en',
  keyboardEnabled: true,
  windowManagementEnabled: true,
  externalWindowActionsEnabled: false,
  commandImeSwitchEnabled: true,
  showInMenuBar: true,
  dragToSnapEnabled: false,
  dragToMoveEnabled: false,
};

const ALL_GRANTED: SetupStatus = {
  firstRun: false,
  updateRegrant: false,
  accessibility: true,
  inputMonitoring: true,
};

function mockCommands(overrides: Record<string, unknown> = {}) {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd in overrides) {
      const value = overrides[cmd];
      return value instanceof Error ? Promise.reject(value) : Promise.resolve(value);
    }
    switch (cmd) {
      case 'get_settings':
        return Promise.resolve(SETTINGS);
      case 'setup_status':
        return Promise.resolve(ALL_GRANTED);
      case 'list_modifier_rules':
      case 'list_hotkeys':
        return Promise.resolve([]);
      case 'input_monitoring_status':
      case 'accessibility_status':
        return Promise.resolve(true);
      default:
        return Promise.resolve(null);
    }
  });
}

describe('App setup flow', () => {
  // Both the shell and the mounted views subscribe to this event, so collect
  // every handler and fan the test's synthetic event out to all of them.
  let permissionHandlers: ((e: { event: string; id: number; payload: unknown }) => void)[] = [];
  const permissionsChanged = (payload: PermissionsChanged) => {
    for (const handler of permissionHandlers) {
      handler({ event: 'tomari:permissions-changed', id: 0, payload });
    }
  };

  beforeEach(() => {
    mockInvoke.mockReset();
    mockCommands();
    permissionHandlers = [];
    mockListen.mockReset();
    mockListen.mockImplementation((event, handler) => {
      if (event === 'tomari:permissions-changed') {
        permissionHandlers.push(handler);
      }
      return Promise.resolve(() => {});
    });
  });

  it('shows the tabs, no checklist and no banner, when nothing is missing', async () => {
    render(<App />);

    expect(await screen.findByText('Keyboard customization')).toBeInTheDocument();
    expect(screen.queryByText('Set up Tomari')).not.toBeInTheDocument();
    expect(screen.queryByText("Setup isn't finished yet.")).not.toBeInTheDocument();
  });

  it('opens the checklist instead of the tabs on a first run with missing permissions', async () => {
    mockCommands({
      setup_status: { ...ALL_GRANTED, firstRun: true, accessibility: false },
    });

    render(<App />);

    expect(await screen.findByText('Set up Tomari')).toBeInTheDocument();
    expect(screen.queryByText('Keyboard customization')).not.toBeInTheDocument();
  });

  it('opens the checklist with the update explanation when an update revoked permissions', async () => {
    mockCommands({
      setup_status: { ...ALL_GRANTED, updateRegrant: true, accessibility: false },
    });

    render(<App />);

    expect(await screen.findByText('Set up Tomari')).toBeInTheDocument();
    expect(screen.getByText(/The update reset these permissions/)).toBeInTheDocument();
  });

  it('shows the tabs plus the reminder banner when permissions are missing on a normal launch', async () => {
    mockCommands({
      setup_status: { ...ALL_GRANTED, inputMonitoring: false },
    });

    render(<App />);

    expect(await screen.findByText("Setup isn't finished yet.")).toBeInTheDocument();
    expect(screen.getByText('Keyboard customization')).toBeInTheDocument();
  });

  it('reopens the checklist from the reminder banner and returns via "Set up later"', async () => {
    mockCommands({
      setup_status: { ...ALL_GRANTED, inputMonitoring: false },
    });

    render(<App />);

    fireEvent.click(await screen.findByText('Continue'));
    expect(await screen.findByText('Set up Tomari')).toBeInTheDocument();

    fireEvent.click(screen.getByText('Set up later'));
    expect(await screen.findByText("Setup isn't finished yet.")).toBeInTheDocument();
  });

  it('retires the reminder banner once the backend reports everything granted', async () => {
    mockCommands({
      setup_status: { ...ALL_GRANTED, inputMonitoring: false },
    });

    render(<App />);
    expect(await screen.findByText("Setup isn't finished yet.")).toBeInTheDocument();

    permissionsChanged({ accessibility: true, inputMonitoring: true });

    await waitFor(() => {
      expect(screen.queryByText("Setup isn't finished yet.")).not.toBeInTheDocument();
    });
  });

  it('brings the reminder banner back when a permission is revoked later', async () => {
    render(<App />);
    expect(await screen.findByText('Keyboard customization')).toBeInTheDocument();
    expect(screen.queryByText("Setup isn't finished yet.")).not.toBeInTheDocument();

    permissionsChanged({ accessibility: true, inputMonitoring: false });

    expect(await screen.findByText("Setup isn't finished yet.")).toBeInTheDocument();
  });

  it('keeps the open checklist up after the last grant so its Done button is seen', async () => {
    mockCommands({
      setup_status: { ...ALL_GRANTED, firstRun: true, inputMonitoring: false },
    });

    render(<App />);
    expect(await screen.findByText('Set up Tomari')).toBeInTheDocument();

    permissionsChanged({ accessibility: true, inputMonitoring: true });

    expect(await screen.findByText('Done')).toBeInTheDocument();
    expect(screen.getByText('Set up Tomari')).toBeInTheDocument();
  });

  it('falls back to the tabs when the setup status cannot be read', async () => {
    mockCommands({
      setup_status: Object.assign(new Error('status unavailable'), { code: 'unknown' }),
    });

    render(<App />);

    expect(await screen.findByText('Keyboard customization')).toBeInTheDocument();
    expect(screen.queryByText("Setup isn't finished yet.")).not.toBeInTheDocument();
  });
});
