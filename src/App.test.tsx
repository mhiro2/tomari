import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { App } from './App';
import type { AppSettings, PermissionsChanged, SetupStatus } from './lib/types';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/app', () => ({ getVersion: vi.fn(() => Promise.resolve('1.2.3')) }));

const { invoke } = await import('@tauri-apps/api/core');
const mockInvoke = vi.mocked(invoke);
const { listen } = await import('@tauri-apps/api/event');
const mockListen = vi.mocked(listen);

const LAST_SECTION_KEY = 'tomari.settings.lastSection';

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
  menuBarTidyEnabled: false,
  menuBarAutoCollapseSecs: 0,
};

const ALL_GRANTED: SetupStatus = {
  firstRun: false,
  updateRegrant: false,
  accessibility: true,
  inputMonitoring: true,
  revision: 0,
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
      case 'save_settings':
        return Promise.resolve({ applyWarnings: [] });
      case 'list_modifier_rules':
      case 'list_hotkeys':
        return Promise.resolve([]);
      case 'get_window_history_status':
        return Promise.resolve({ canUndo: false, canRedo: false });
      case 'input_monitoring_status':
      case 'accessibility_status':
        return Promise.resolve(true);
      case 'get_keep_awake':
        return Promise.resolve({
          active: false,
          lidClose: 'off',
          phase: 'off',
          options: {
            durationSecs: null,
            endsAtMs: null,
            acOnly: false,
            lowBatteryAction: 'warn',
          },
          notice: null,
          powerSource: 'ac',
          batteryPercent: 80,
          kernelSleepDisabled: false,
          ownsLidClose: false,
          leftoverUndecided: false,
          longRunningProcesses: [],
          revision: 1,
        });
      default:
        return Promise.resolve(null);
    }
  });
}

function sidebar() {
  return screen.getByRole('navigation', { name: 'Sections' });
}

function nav(name: string) {
  return within(sidebar()).getByRole('button', { name });
}

describe('App setup and permission status', () => {
  let permissionHandlers: ((event: { event: string; id: number; payload: unknown }) => void)[] = [];

  function permissionsChanged(payload: PermissionsChanged) {
    act(() => {
      for (const handler of permissionHandlers) {
        handler({ event: 'tomari:permissions-changed', id: 0, payload });
      }
    });
  }

  beforeEach(() => {
    window.localStorage.clear();
    mockInvoke.mockReset();
    mockCommands();
    permissionHandlers = [];
    mockListen.mockReset();
    mockListen.mockImplementation((event, handler) => {
      if (event === 'tomari:permissions-changed') permissionHandlers.push(handler);
      return Promise.resolve(() => {});
    });
  });

  it('opens the Windows page with a ready permission footer', async () => {
    render(<App />);

    expect(await screen.findByRole('heading', { name: 'Windows', level: 1 })).toBeInTheDocument();
    expect(nav('Windows')).toHaveAttribute('aria-current', 'page');
    expect(within(sidebar()).getByText('Permissions: Ready')).toBeInTheDocument();
    expect(screen.queryByRole('dialog', { name: 'Get Tomari ready' })).not.toBeInTheDocument();
    expect(within(sidebar()).queryByRole('button', { name: 'Home' })).not.toBeInTheDocument();
  });

  it('opens setup immediately on the first run when a permission is missing', async () => {
    mockCommands({
      setup_status: { ...ALL_GRANTED, firstRun: true, accessibility: false },
    });

    render(<App />);

    expect(await screen.findByRole('dialog', { name: 'Get Tomari ready' })).toBeInTheDocument();
    expect(screen.getByText('Accessibility')).toBeInTheDocument();
    expect(nav('Permissions: Needs attention')).toBeInTheDocument();
  });

  it('keeps a normal launch on the current page and opens setup from the permission footer', async () => {
    mockCommands({
      setup_status: { ...ALL_GRANTED, inputMonitoring: false },
    });

    render(<App />);

    expect(await screen.findByRole('heading', { name: 'Windows', level: 1 })).toBeInTheDocument();
    expect(screen.queryByRole('dialog', { name: 'Get Tomari ready' })).not.toBeInTheDocument();

    fireEvent.click(nav('Permissions: Needs attention'));
    expect(await screen.findByRole('dialog', { name: 'Get Tomari ready' })).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Set up later' }));
    await waitFor(() => {
      expect(screen.queryByRole('dialog', { name: 'Get Tomari ready' })).not.toBeInTheDocument();
    });
    expect(nav('Windows')).toHaveAttribute('aria-current', 'page');
    expect(nav('Permissions: Needs attention')).toBeInTheDocument();
  });

  it('updates the footer when permissions change outside Tomari', async () => {
    render(<App />);
    expect(await screen.findByText('Permissions: Ready')).toBeInTheDocument();

    permissionsChanged({ accessibility: true, inputMonitoring: false, revision: 1 });
    expect(
      await screen.findByRole('button', { name: 'Permissions: Needs attention' }),
    ).toBeInTheDocument();

    permissionsChanged({ accessibility: true, inputMonitoring: true, revision: 2 });
    expect(await screen.findByText('Permissions: Ready')).toBeInTheDocument();
  });

  it('preserves the update explanation only for the update recovery flow', async () => {
    mockCommands({
      setup_status: { ...ALL_GRANTED, updateRegrant: true, accessibility: false },
    });

    render(<App />);
    expect(await screen.findByText(/went missing after the update/)).toBeInTheDocument();

    permissionsChanged({ accessibility: true, inputMonitoring: true, revision: 3 });
    fireEvent.click(await screen.findByRole('button', { name: 'Start using Tomari' }));

    permissionsChanged({ accessibility: false, inputMonitoring: true, revision: 4 });
    fireEvent.click(await screen.findByRole('button', { name: 'Permissions: Needs attention' }));

    expect(await screen.findByRole('dialog', { name: 'Get Tomari ready' })).toBeInTheDocument();
    expect(screen.queryByText(/went missing after the update/)).not.toBeInTheDocument();
  });

  it('falls back to Windows when setup status cannot be read', async () => {
    mockCommands({
      setup_status: Object.assign(new Error('status unavailable'), { code: 'unknown' }),
    });

    render(<App />);

    expect(await screen.findByRole('heading', { name: 'Windows', level: 1 })).toBeInTheDocument();
    // Unknown is not ready: the status says so and offers a retry.
    const status = within(sidebar()).getByRole('button', { name: 'Permissions: Checking…' });
    mockCommands();
    fireEvent.click(status);
    expect(await within(sidebar()).findByText('Permissions: Ready')).toBeInTheDocument();
    expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === 'setup_status')).toHaveLength(2);
  });

  it('does not pull the status until the transition listener is registered', async () => {
    let finishListen: (() => void) | undefined;
    mockListen.mockImplementation((event, handler) => {
      if (event === 'tomari:permissions-changed') {
        permissionHandlers.push(handler);
        return new Promise<() => void>((resolve) => {
          finishListen = () => resolve(() => {});
        });
      }
      return Promise.resolve(() => {});
    });
    render(<App />);
    await waitFor(() => expect(permissionHandlers).toHaveLength(1));
    expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === 'setup_status')).toHaveLength(0);

    await act(async () => {
      finishListen?.();
    });
    await waitFor(() =>
      expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === 'setup_status')).toHaveLength(1),
    );
  });

  it('still loads and pulls the status once when the listener cannot be registered', async () => {
    mockListen.mockImplementation((event) =>
      event === 'tomari:permissions-changed'
        ? Promise.reject(new Error('no event bridge'))
        : Promise.resolve(() => {}),
    );
    render(<App />);
    expect(await screen.findByRole('heading', { name: 'Windows', level: 1 })).toBeInTheDocument();
    expect(within(sidebar()).getByText('Permissions: Ready')).toBeInTheDocument();
    expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === 'setup_status')).toHaveLength(1);
  });

  it('keeps the first snapshot of a revision and ignores a later one with the same revision', async () => {
    let resolveStatus: ((status: SetupStatus) => void) | undefined;
    mockCommands({
      setup_status: new Promise<SetupStatus>((resolve) => {
        resolveStatus = resolve;
      }),
    });
    render(<App />);
    await waitFor(() => expect(permissionHandlers).toHaveLength(1));

    permissionsChanged({ accessibility: false, inputMonitoring: true, revision: 3 });
    await act(async () => {
      resolveStatus?.({ ...ALL_GRANTED, revision: 3 });
    });
    expect(
      within(sidebar()).getByRole('button', { name: 'Permissions: Needs attention' }),
    ).toBeInTheDocument();
  });

  it('decides the setup dialog from the winning snapshot, not the losing pull', async () => {
    // First run: the pull says everything is missing, but a newer event says
    // it has all been granted meanwhile — no dialog.
    let resolveStatus: ((status: SetupStatus) => void) | undefined;
    mockCommands({
      setup_status: new Promise<SetupStatus>((resolve) => {
        resolveStatus = resolve;
      }),
    });
    render(<App />);
    await waitFor(() => expect(permissionHandlers).toHaveLength(1));
    permissionsChanged({ accessibility: true, inputMonitoring: true, revision: 9 });
    await act(async () => {
      resolveStatus?.({
        ...ALL_GRANTED,
        firstRun: true,
        accessibility: false,
        inputMonitoring: false,
        revision: 8,
      });
    });
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(within(sidebar()).getByText('Permissions: Ready')).toBeInTheDocument();

    // A later, unrelated revoke opens the dialog without the stale "lost after
    // the update" explanation the losing pull carried.
    permissionsChanged({ accessibility: false, inputMonitoring: true, revision: 10 });
    fireEvent.click(
      within(sidebar()).getByRole('button', { name: 'Permissions: Needs attention' }),
    );
    expect(await screen.findByRole('dialog')).toBeInTheDocument();
    expect(screen.queryByText(/went missing after the update/)).not.toBeInTheDocument();
  });

  it('lets a transition that lands during the initial pull win over the pull', async () => {
    // The listener is registered before the pull, so an event arriving while
    // the pull is in flight is seen; its higher revision means it is newer
    // than the pull's snapshot, which must not overwrite it when it lands.
    let resolveStatus: ((status: SetupStatus) => void) | undefined;
    mockCommands({
      setup_status: new Promise<SetupStatus>((resolve) => {
        resolveStatus = resolve;
      }),
    });
    render(<App />);
    await waitFor(() => expect(permissionHandlers).toHaveLength(1));

    permissionsChanged({ accessibility: false, inputMonitoring: true, revision: 5 });
    await act(async () => {
      resolveStatus?.({ ...ALL_GRANTED, revision: 4 });
    });
    expect(
      within(sidebar()).getByRole('button', { name: 'Permissions: Needs attention' }),
    ).toBeInTheDocument();
  });
});

describe('App sidebar and page persistence', () => {
  beforeEach(() => {
    window.localStorage.clear();
    mockInvoke.mockReset();
    mockCommands();
    mockListen.mockReset();
    mockListen.mockImplementation(() => Promise.resolve(() => {}));
  });

  it('shows only concise tool and app destinations in separate groups', async () => {
    render(<App />);
    await screen.findByRole('heading', { name: 'Windows', level: 1 });

    const tools = within(sidebar()).getByRole('region', { name: 'Tools' });
    const app = within(sidebar()).getByRole('region', { name: 'App' });

    expect(
      within(tools)
        .getAllByRole('button')
        .map((button) => button.textContent),
    ).toEqual(['Windows', 'Keyboard', 'Menu Bar', 'Prevent Sleep']);
    expect(
      within(app)
        .getAllByRole('button')
        .map((button) => button.textContent),
    ).toEqual(['General']);
    expect(within(sidebar()).queryByRole('button', { name: 'Home' })).not.toBeInTheDocument();
  });

  it('does not add feature state to a sidebar destination name', async () => {
    mockCommands({ get_settings: { ...SETTINGS, windowManagementEnabled: false } });

    render(<App />);
    await screen.findByRole('heading', { name: 'Windows', level: 1 });

    expect(nav('Windows')).toBeInTheDocument();
    expect(
      within(sidebar()).queryByRole('button', { name: 'Windows (Off)' }),
    ).not.toBeInTheDocument();
  });

  it('persists the selected page and restores it after remounting', async () => {
    const first = render(<App />);
    await screen.findByRole('heading', { name: 'Windows', level: 1 });

    fireEvent.click(nav('Keyboard'));
    expect(await screen.findByRole('heading', { name: 'Keyboard', level: 1 })).toBeInTheDocument();
    await waitFor(() => {
      expect(window.localStorage.getItem(LAST_SECTION_KEY)).toBe('keyboard');
    });

    first.unmount();
    render(<App />);

    expect(await screen.findByRole('heading', { name: 'Keyboard', level: 1 })).toBeInTheDocument();
    expect(nav('Keyboard')).toHaveAttribute('aria-current', 'page');
  });

  it('falls back to Windows for an invalid or removed saved page', async () => {
    window.localStorage.setItem(LAST_SECTION_KEY, 'overview');

    render(<App />);

    expect(await screen.findByRole('heading', { name: 'Windows', level: 1 })).toBeInTheDocument();
    expect(nav('Windows')).toHaveAttribute('aria-current', 'page');
    expect(window.localStorage.getItem(LAST_SECTION_KEY)).toBe('window');
  });

  it('switches pages and moves the current marker', async () => {
    render(<App />);
    await screen.findByRole('heading', { name: 'Windows', level: 1 });

    fireEvent.click(nav('Prevent Sleep'));
    expect(
      await screen.findByRole('heading', { name: 'Prevent Sleep', level: 1 }),
    ).toBeInTheDocument();
    expect(nav('Prevent Sleep')).toHaveAttribute('aria-current', 'page');
    expect(nav('Windows')).not.toHaveAttribute('aria-current');

    fireEvent.click(nav('General'));
    expect(await screen.findByRole('heading', { name: 'General', level: 1 })).toBeInTheDocument();
    expect(nav('General')).toHaveAttribute('aria-current', 'page');
  });

  it('keeps a partial-apply warning visible outside General', async () => {
    mockCommands({ save_settings: { applyWarnings: ['launchAtLogin'] } });
    render(<App />);
    await screen.findByRole('heading', { name: 'Windows', level: 1 });

    fireEvent.click(nav('General'));
    fireEvent.click(await screen.findByRole('switch', { name: 'Launch at login' }));
    expect(await screen.findByText('Saved, but not fully applied')).toBeInTheDocument();

    fireEvent.click(nav('Windows'));
    expect(await screen.findByText(/macOS could not apply part/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Review' }));
    expect(await screen.findByRole('heading', { name: 'General', level: 1 })).toBeInTheDocument();
  });
});
