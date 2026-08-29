import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { App } from './App';
import type {
  AppSettings,
  ConfigurationWarnings,
  PermissionsChanged,
  SetupStatus,
} from './lib/types';

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

const CONFIGURATION_WARNINGS: ConfigurationWarnings = {
  invalidHotkeys: [
    {
      id: 'legacy-plain-key',
      label: 'Legacy quick panel',
      reason: 'unsafeGlobalShortcut',
    },
  ],
  invalidModifierRules: [
    {
      id: 'legacy-command',
      label: 'Left Command override',
      reason: 'reservedCommandSlot',
    },
  ],
  revision: 3,
};

const RECOVERY_ERROR = {
  code: 'settingsRecoveryRequired',
  message: 'settings row does not decode',
};

const DATABASE_RESET_ERROR = {
  code: 'databaseResetRequired',
  message: 'settings database was quarantined',
};

function defaultCommand(cmd: string): Promise<unknown> {
  switch (cmd) {
    case 'get_settings':
      return Promise.resolve(SETTINGS);
    case 'setup_status':
      return Promise.resolve(ALL_GRANTED);
    case 'save_settings':
      return Promise.resolve({ applyWarnings: [] });
    case 'get_configuration_warnings':
      return Promise.resolve({ invalidHotkeys: [], invalidModifierRules: [], revision: 0 });
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
}

function mockCommands(overrides: Record<string, unknown> = {}) {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd in overrides) {
      const value = overrides[cmd];
      return value instanceof Error ? Promise.reject(value) : Promise.resolve(value);
    }
    return defaultCommand(cmd);
  });
}

function mockRecoveryCommands({
  retry,
  reset,
  recoveredSettings = SETTINGS,
  initialError = RECOVERY_ERROR,
}: {
  retry?: Error | 'recover';
  reset?: Error | 'recover';
  recoveredSettings?: AppSettings;
  initialError?: unknown;
} = {}) {
  let recovered = false;
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === 'get_settings') {
      return recovered ? Promise.resolve(recoveredSettings) : Promise.reject(initialError);
    }
    if (cmd === 'retry_settings_recovery') {
      if (retry instanceof Error) return Promise.reject(retry);
      if (retry === 'recover') {
        recovered = true;
        return Promise.resolve();
      }
      return new Promise<void>(() => {});
    }
    if (cmd === 'reset_settings_recovery') {
      if (reset instanceof Error) return Promise.reject(reset);
      if (reset === 'recover') {
        recovered = true;
        return Promise.resolve();
      }
      return new Promise<void>(() => {});
    }
    return defaultCommand(cmd);
  });
}

function setNavigatorLanguage(value: string) {
  Object.defineProperty(window.navigator, 'language', { value, configurable: true });
}

function sidebar() {
  return screen.getByRole('navigation', { name: 'Sections' });
}

function nav(name: string) {
  return within(sidebar()).getByRole('button', { name });
}

describe('App settings recovery', () => {
  const originalLanguage = window.navigator.language;

  beforeEach(() => {
    window.localStorage.clear();
    mockInvoke.mockReset();
    mockRecoveryCommands();
    mockListen.mockReset();
    mockListen.mockImplementation(() => Promise.resolve(() => {}));
    setNavigatorLanguage('en-US');
  });

  afterEach(() => {
    setNavigatorLanguage(originalLanguage);
  });

  it('keeps the operational shell unmounted while the initial settings read is pending', () => {
    mockInvoke.mockImplementation((cmd: string) =>
      cmd === 'get_settings' ? new Promise<AppSettings>(() => {}) : defaultCommand(cmd),
    );

    render(<App />);

    expect(screen.getByRole('status')).toHaveTextContent('Loading…');
    expect(screen.queryByRole('navigation')).not.toBeInTheDocument();
    expect(mockInvoke.mock.calls.map(([cmd]) => cmd)).toEqual(['get_settings']);
  });

  it('mounts only the focused recovery surface when the initial settings read requires repair', async () => {
    render(<App />);

    const heading = await screen.findByRole('heading', { name: 'Settings need repair', level: 1 });
    expect(heading).toHaveFocus();
    expect(screen.getByRole('alert')).toHaveTextContent('Automation is paused');
    expect(screen.getByRole('main').parentElement).toHaveAttribute('aria-busy', 'false');
    expect(screen.queryByRole('navigation')).not.toBeInTheDocument();
    expect(screen.queryByRole('dialog', { name: 'Get Tomari ready' })).not.toBeInTheDocument();

    const commands = mockInvoke.mock.calls.map(([cmd]) => cmd);
    expect(commands).toEqual(['get_settings']);
    expect(commands).not.toContain('setup_status');
    expect(commands).not.toContain('list_hotkeys');
    expect(commands).not.toContain('get_apply_warnings');
  });

  it('keeps recovery active and reports a retry failure locally', async () => {
    mockRecoveryCommands({ retry: new Error('read still failed') });
    render(<App />);
    await screen.findByRole('heading', { name: 'Settings need repair' });

    fireEvent.click(screen.getByRole('button', { name: 'Try Again' }));

    expect(
      await screen.findByText('Tomari still could not read the settings: read still failed'),
    ).toHaveAttribute('role', 'alert');
    expect(screen.getByRole('heading', { name: 'Settings need repair' })).toBeInTheDocument();
    expect(screen.queryByRole('navigation')).not.toBeInTheDocument();
  });

  it('offers only an explicit reset after a damaged database was quarantined', async () => {
    mockRecoveryCommands({
      initialError: DATABASE_RESET_ERROR,
      reset: new Error('replacement failed'),
    });
    render(<App />);

    await screen.findByRole('heading', { name: 'Settings need repair' });
    expect(
      screen.getByText(
        'Tomari found a damaged settings database and preserved it for manual recovery.',
      ),
    ).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Reset to continue' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Try Again' })).not.toBeInTheDocument();

    const reset = screen.getByRole('button', { name: 'Reset Settings…' });
    expect(reset).toHaveClass('btn--primary');
    fireEvent.click(reset);
    const confirmation = screen.getByRole('group', { name: 'Reset unreadable settings?' });
    fireEvent.click(within(confirmation).getByRole('button', { name: 'Reset and Continue' }));
    expect(
      await screen.findByText('Tomari could not reset the settings: replacement failed'),
    ).toHaveAttribute('role', 'alert');
    expect(screen.queryByRole('button', { name: 'Try Again' })).not.toBeInTheDocument();
    expect(mockInvoke.mock.calls.some(([cmd]) => cmd === 'retry_settings_recovery')).toBe(false);
  });

  it('requires confirmation and supports both Escape and Cancel without resetting', async () => {
    render(<App />);
    await screen.findByRole('heading', { name: 'Settings need repair' });

    const reset = screen.getByRole('button', { name: 'Reset Settings…' });
    fireEvent.click(reset);

    let confirmation = screen.getByRole('group', { name: 'Reset unreadable settings?' });
    expect(
      within(confirmation).getByText(/Shortcuts or rules may be replaced/),
    ).toBeInTheDocument();
    expect(
      within(confirmation).getByText(/Automation stays off until you turn each feature back on/),
    ).toBeInTheDocument();
    const confirmReset = within(confirmation).getByRole('button', {
      name: 'Reset and Continue',
    });
    expect(confirmReset).toHaveFocus();
    expect(mockInvoke.mock.calls.some(([cmd]) => cmd === 'reset_settings_recovery')).toBe(false);

    // The confirmation stays dismissible after focus moves back outside its
    // own controls, for example via Shift+Tab.
    reset.focus();
    fireEvent.keyDown(reset, { key: 'Escape' });
    expect(
      screen.queryByRole('group', { name: 'Reset unreadable settings?' }),
    ).not.toBeInTheDocument();
    expect(reset).toHaveFocus();

    fireEvent.click(reset);
    confirmation = screen.getByRole('group', { name: 'Reset unreadable settings?' });
    fireEvent.click(within(confirmation).getByRole('button', { name: 'Cancel' }));
    expect(
      screen.queryByRole('group', { name: 'Reset unreadable settings?' }),
    ).not.toBeInTheDocument();
    expect(reset).toHaveFocus();
    expect(mockInvoke.mock.calls.some(([cmd]) => cmd === 'reset_settings_recovery')).toBe(false);
  });

  it('keeps the confirmation visible and reports a reset failure locally', async () => {
    mockRecoveryCommands({ reset: new Error('replacement failed') });
    render(<App />);
    await screen.findByRole('heading', { name: 'Settings need repair' });

    fireEvent.click(screen.getByRole('button', { name: 'Reset Settings…' }));
    fireEvent.click(screen.getByRole('button', { name: 'Reset and Continue' }));

    expect(
      await screen.findByText('Tomari could not reset the settings: replacement failed'),
    ).toHaveAttribute('role', 'alert');
    expect(screen.getByRole('group', { name: 'Reset unreadable settings?' })).toBeInTheDocument();
    expect(screen.queryByRole('navigation')).not.toBeInTheDocument();
  });

  it('reloads a healthy snapshot when a mocked reset command resolves', async () => {
    const safeSettings: AppSettings = {
      ...SETTINGS,
      keyboardEnabled: false,
      windowManagementEnabled: false,
      commandImeSwitchEnabled: false,
    };
    mockRecoveryCommands({ reset: 'recover', recoveredSettings: safeSettings });
    render(<App />);
    await screen.findByRole('heading', { name: 'Settings need repair' });

    fireEvent.click(screen.getByRole('button', { name: 'Reset Settings…' }));
    fireEvent.click(screen.getByRole('button', { name: 'Reset and Continue' }));

    expect(await screen.findByRole('heading', { name: 'Windows', level: 1 })).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Settings need repair' })).not.toBeInTheDocument();
    expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === 'get_settings')).toHaveLength(2);
    expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === 'reset_settings_recovery')).toHaveLength(
      1,
    );
    expect(
      mockInvoke.mock.calls.find(([cmd]) => cmd === 'reset_settings_recovery')?.[1],
    ).toBeUndefined();
    expect(mockInvoke.mock.calls.some(([cmd]) => cmd === 'save_settings')).toBe(false);
  });

  it('renders the recovery contract in Japanese for a Japanese system locale', async () => {
    setNavigatorLanguage('ja-JP');
    render(<App />);

    expect(
      await screen.findByRole('heading', { name: '設定の修復が必要です', level: 1 }),
    ).toBeInTheDocument();
    expect(screen.getByRole('alert')).toHaveTextContent('自動操作を停止しています');
    expect(screen.getByRole('button', { name: 'もう一度読み込む' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '設定をリセット…' })).toBeInTheDocument();
  });
});

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

  it('keeps the operational loading state centered while setup status is pending', async () => {
    let resolveSetup: ((status: SetupStatus) => void) | undefined;
    const pendingSetup = new Promise<SetupStatus>((resolve) => {
      resolveSetup = resolve;
    });
    mockCommands({ setup_status: pendingSetup });

    render(<App />);

    await waitFor(() =>
      expect(mockInvoke.mock.calls.some(([cmd]) => cmd === 'setup_status')).toBe(true),
    );
    const loading = screen.getByRole('status');
    expect(loading).toHaveClass('app', 'app--loading');
    expect(loading).toHaveTextContent('Loading…');
    expect(screen.queryByRole('navigation')).not.toBeInTheDocument();

    await act(async () => {
      resolveSetup?.(ALL_GRANTED);
    });
    expect(await screen.findByRole('heading', { name: 'Windows', level: 1 })).toBeInTheDocument();
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

  it('keeps configuration warnings in the shell and moves focus to Keyboard details', async () => {
    mockCommands({ get_configuration_warnings: CONFIGURATION_WARNINGS });
    render(<App />);
    await screen.findByRole('heading', { name: 'Windows', level: 1 });

    expect(await screen.findByRole('status')).toHaveTextContent(
      'Saved keyboard items need attention (2)',
    );
    expect(
      screen.getByText(
        'Invalid saved items were not deleted. Keyboard items without problems continue to work.',
      ),
    ).toBeInTheDocument();

    fireEvent.click(nav('General'));
    await screen.findByRole('heading', { name: 'General', level: 1 });
    expect(screen.getByText('Saved keyboard items need attention (2)')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Review in Keyboard' }));
    const issueHeading = await screen.findByRole('heading', {
      name: 'Some saved keyboard items are not running',
      level: 2,
    });
    expect(nav('Keyboard')).toHaveAttribute('aria-current', 'page');
    expect(issueHeading).toHaveFocus();
    expect(screen.getByText('Legacy quick panel')).toBeInTheDocument();
    expect(screen.getByText('The shortcut could intercept ordinary typing.')).toBeInTheDocument();
    expect(screen.getByText('Left Command override')).toBeInTheDocument();
    expect(
      screen.getByText('This Command-key slot is reserved by the built-in input switch.'),
    ).toBeInTheDocument();
  });

  it('localizes configuration warning safety copy and reasons in Japanese', async () => {
    mockCommands({
      get_settings: { ...SETTINGS, language: 'ja' },
      get_configuration_warnings: CONFIGURATION_WARNINGS,
    });
    render(<App />);

    expect(
      await screen.findByText('確認が必要なキーボード項目があります（2 件）'),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        '無効な保存済み項目は削除されていません。問題のないキーボード項目は引き続き動作します。',
      ),
    ).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'キーボードで確認' }));
    expect(
      await screen.findByText('通常の文字入力を奪う可能性があるショートカットです。'),
    ).toBeInTheDocument();
    expect(
      screen.getByText('組み込みの入力切り替えが使用する Command キーです。'),
    ).toBeInTheDocument();
  });

  it('bounds warning labels and strips control and bidirectional format characters', async () => {
    mockCommands({
      get_configuration_warnings: {
        invalidHotkeys: [
          {
            id: 'unsafe-label',
            label: `Safe\u0000\u202e${'x'.repeat(120)}`,
            reason: 'unsafeGlobalShortcut',
          },
        ],
        invalidModifierRules: [],
        revision: 1,
      } satisfies ConfigurationWarnings,
    });
    render(<App />);

    fireEvent.click(await screen.findByRole('button', { name: 'Review in Keyboard' }));
    const label = await screen.findByText(/^Safe x+…$/u);
    expect(label.textContent).not.toMatch(/[\p{Cc}\p{Cf}]/u);
    expect(Array.from(label.textContent ?? '')).toHaveLength(97);
  });
});
