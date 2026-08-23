import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { ReactElement } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { SettingsProvider } from '../lib/settings';
import type { AppSettings, MenuBarInventory, MenuBarStatus } from '../lib/types';
import { MenuBarView } from './MenuBarView';

// Mock the Tauri command bridge so the real `api` wrappers run against it.
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
const { invoke } = await import('@tauri-apps/api/core');
const mockInvoke = vi.mocked(invoke);

// vitest.setup.ts stubs `listen` as a permanent no-op; capture the callback so
// tests can drive "tomari:menu-bar-changed" the way the backend would.
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
  menuBarTidyEnabled: true,
  menuBarAutoCollapseSecs: 0,
};

const INVENTORY: MenuBarInventory = {
  supported: true,
  permissionGranted: true,
  dividerAvailable: true,
  items: [
    {
      id: 'com.docker.docker:status:0',
      name: 'Docker',
      ownerName: null,
      bundleId: 'com.docker.docker',
      zone: 'hidden',
    },
    {
      id: 'com.apple.controlcenter:wifi:0',
      name: 'Wi-Fi',
      ownerName: 'Control Center',
      bundleId: 'com.apple.controlcenter',
      zone: 'visible',
    },
    {
      id: 'com.apple.controlcenter:battery:1',
      name: 'Battery',
      ownerName: 'Control Center',
      bundleId: 'com.apple.controlcenter',
      zone: 'visible',
    },
  ],
};

function renderView(ui: ReactElement) {
  return render(<SettingsProvider>{ui}</SettingsProvider>);
}

async function openBehavior() {
  fireEvent.click(await screen.findByRole('tab', { name: 'Behavior' }));
}

function mockCommands(overrides: Record<string, unknown> = {}) {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd in overrides) {
      const value = overrides[cmd];
      return value instanceof Error ? Promise.reject(value) : Promise.resolve(value);
    }
    switch (cmd) {
      case 'get_settings':
        return Promise.resolve(SETTINGS);
      case 'get_menu_bar':
        return Promise.resolve({ enabled: true, collapsed: true } satisfies MenuBarStatus);
      case 'list_menu_bar_items':
        return Promise.resolve(INVENTORY);
      case 'save_settings':
        return Promise.resolve({ applyWarnings: [] });
      case 'set_menu_bar_collapsed':
        return Promise.resolve({ enabled: true, collapsed: false } satisfies MenuBarStatus);
      default:
        return Promise.resolve(null);
    }
  });
}

describe('MenuBarView', () => {
  let menuBarChanged: ((payload: MenuBarStatus) => void) | undefined;

  beforeEach(() => {
    mockInvoke.mockReset();
    mockCommands();
    menuBarChanged = undefined;
    mockListen.mockReset();
    mockListen.mockImplementation((event, handler) => {
      if (event === 'tomari:menu-bar-changed') {
        menuBarChanged = (payload) =>
          (handler as (e: { event: string; id: number; payload: unknown }) => void)({
            event,
            id: 0,
            payload,
          });
      }
      return Promise.resolve(() => {});
    });
  });

  it('reflects the collapsed state pulled on mount', async () => {
    renderView(<MenuBarView />);
    await openBehavior();

    expect((await screen.findAllByText('Hidden icons are tucked away')).length).toBeGreaterThan(0);
    expect(screen.getByRole('button', { name: 'Show icons' })).toBeInTheDocument();
  });

  it('registers the runtime listener before pulling the initial status', async () => {
    let finishRegistration: (() => void) | undefined;
    mockListen.mockImplementation((event, handler) => {
      if (event !== 'tomari:menu-bar-changed') return Promise.resolve(() => {});
      return new Promise((resolve) => {
        finishRegistration = () => {
          menuBarChanged = (payload) =>
            (handler as (e: { event: string; id: number; payload: unknown }) => void)({
              event,
              id: 0,
              payload,
            });
          resolve(() => {});
        };
      });
    });

    renderView(<MenuBarView />);
    await screen.findByRole('heading', { name: 'Menu Bar' });
    await waitFor(() =>
      expect(mockListen).toHaveBeenCalledWith('tomari:menu-bar-changed', expect.any(Function)),
    );

    expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === 'get_menu_bar')).toHaveLength(0);
    await act(async () => finishRegistration?.());
    await waitFor(() =>
      expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === 'get_menu_bar')).toHaveLength(1),
    );
  });

  it('expands through the backend when the action button is used', async () => {
    renderView(<MenuBarView />);
    await openBehavior();

    fireEvent.click(await screen.findByRole('button', { name: 'Show icons' }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('set_menu_bar_collapsed', { collapsed: false });
    });
    expect((await screen.findAllByText('Hidden icons are visible now')).length).toBeGreaterThan(0);
    expect(screen.getByRole('button', { name: 'Hide again' })).toBeInTheDocument();
  });

  it('follows a change made from the menu bar item or the tray', async () => {
    renderView(<MenuBarView />);
    await screen.findByRole('heading', { name: 'Menu Bar' });

    act(() => menuBarChanged?.({ enabled: true, collapsed: false }));
    await openBehavior();

    await waitFor(() => {
      expect(screen.getAllByText('Hidden icons are visible now').length).toBeGreaterThan(0);
    });
  });

  it('keeps the last known state when the backend rejects the change', async () => {
    mockCommands({
      set_menu_bar_collapsed: Object.assign(new Error('nope'), { code: 'unknown' }),
    });
    renderView(<MenuBarView />);
    await openBehavior();

    fireEvent.click(await screen.findByRole('button', { name: 'Show icons' }));

    // Still collapsed: the backend owns the state, so a failed call must not
    // leave the action showing something that never happened.
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Show icons' })).toBeInTheDocument();
    });
  });

  it('persists the auto-collapse delay', async () => {
    renderView(<MenuBarView />);
    await openBehavior();

    fireEvent.change(await screen.findByLabelText('Collapse automatically'), {
      target: { value: '15' },
    });

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('save_settings', {
        settings: expect.objectContaining({ menuBarAutoCollapseSecs: 15 }),
      });
    });
  });

  it('shows the real hidden and visible menu bar inventory', async () => {
    renderView(<MenuBarView />);

    expect(await screen.findByText('Docker')).toBeInTheDocument();
    expect(screen.getByText('Wi-Fi')).toBeInTheDocument();
    expect(screen.getByText('Battery')).toBeInTheDocument();
    expect(screen.getAllByText('From Control Center')).toHaveLength(2);
    expect(screen.getByText('Hidden now')).toBeInTheDocument();
    expect(screen.getByText('Always shown')).toBeInTheDocument();
  });

  it('refreshes the inventory after a Command-drag arrangement change', async () => {
    renderView(<MenuBarView />);
    const refresh = await screen.findByRole('button', { name: 'Refresh Items' });

    fireEvent.click(refresh);

    await waitFor(() => {
      expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === 'list_menu_bar_items')).toHaveLength(
        2,
      );
    });
  });

  it('shows why inventory is unavailable without duplicating the permission action', async () => {
    mockCommands({
      list_menu_bar_items: {
        supported: true,
        permissionGranted: false,
        dividerAvailable: false,
        items: [],
      } satisfies MenuBarInventory,
    });
    renderView(<MenuBarView />);

    expect(
      await screen.findByText('Accessibility access is required to identify menu bar items.'),
    ).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Grant Access…' })).not.toBeInTheDocument();
  });

  it('keeps the item diagram visible but disabled when the feature is off', async () => {
    mockCommands({
      get_settings: { ...SETTINGS, menuBarTidyEnabled: false },
      get_menu_bar: { enabled: false, collapsed: true } satisfies MenuBarStatus,
    });
    renderView(<MenuBarView />);

    expect(await screen.findByText('Current menu bar arrangement')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Refresh Items' })).toBeDisabled();
    expect(screen.getByRole('tab', { name: 'Behavior' })).toBeEnabled();

    fireEvent.click(screen.getByRole('switch', { name: 'Turn on menu bar tidying' }));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('save_settings', {
        settings: expect.objectContaining({ menuBarTidyEnabled: true }),
      });
    });
  });

  it('waits for the confirmed runtime enable before scanning the inventory', async () => {
    mockCommands({
      get_settings: { ...SETTINGS, menuBarTidyEnabled: false },
      get_menu_bar: { enabled: false, collapsed: true } satisfies MenuBarStatus,
    });
    renderView(<MenuBarView />);

    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('get_menu_bar'));
    expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === 'list_menu_bar_items')).toHaveLength(0);

    fireEvent.click(await screen.findByRole('switch', { name: 'Turn on menu bar tidying' }));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('save_settings', {
        settings: expect.objectContaining({ menuBarTidyEnabled: true }),
      });
    });

    // The optimistic settings value must not scan before the backend has
    // applied the divider and published its runtime status.
    expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === 'list_menu_bar_items')).toHaveLength(0);

    act(() => menuBarChanged?.({ enabled: true, collapsed: false }));

    expect(await screen.findByText('Docker')).toBeInTheDocument();
    expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === 'list_menu_bar_items')).toHaveLength(1);
    expect(screen.queryByText('Tomari’s divider is not available yet.')).not.toBeInTheDocument();
  });
});
