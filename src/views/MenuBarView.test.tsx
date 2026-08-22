import { fireEvent, render, screen, waitFor } from '@testing-library/react';
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
  ],
};

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
      case 'get_settings':
        return Promise.resolve(SETTINGS);
      case 'get_menu_bar':
        return Promise.resolve({ enabled: true, collapsed: true } satisfies MenuBarStatus);
      case 'list_menu_bar_items':
        return Promise.resolve(INVENTORY);
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

    const toggle = await screen.findByRole('switch', { name: 'Show hidden icons' });
    expect(toggle).toHaveAttribute('aria-checked', 'false');
  });

  it('expands through the backend when the switch is turned on', async () => {
    renderView(<MenuBarView />);

    fireEvent.click(await screen.findByRole('switch', { name: 'Show hidden icons' }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('set_menu_bar_collapsed', { collapsed: false });
    });
    expect(await screen.findByRole('switch', { name: 'Show hidden icons' })).toHaveAttribute(
      'aria-checked',
      'true',
    );
  });

  it('follows a change made from the menu bar item or the tray', async () => {
    renderView(<MenuBarView />);
    await screen.findByRole('switch', { name: 'Show hidden icons' });

    menuBarChanged?.({ enabled: true, collapsed: false });

    await waitFor(() => {
      expect(screen.getByRole('switch', { name: 'Show hidden icons' })).toHaveAttribute(
        'aria-checked',
        'true',
      );
    });
  });

  it('keeps the last known state when the backend rejects the change', async () => {
    mockCommands({
      set_menu_bar_collapsed: Object.assign(new Error('nope'), { code: 'unknown' }),
    });
    renderView(<MenuBarView />);

    fireEvent.click(await screen.findByRole('switch', { name: 'Show hidden icons' }));

    // Still collapsed: the backend owns the state, so a failed call must not
    // leave the switch showing something that never happened.
    await waitFor(() => {
      expect(screen.getByRole('switch', { name: 'Show hidden icons' })).toHaveAttribute(
        'aria-checked',
        'false',
      );
    });
  });

  it('persists the auto-collapse delay', async () => {
    renderView(<MenuBarView />);

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
    expect(screen.getByText('From Control Center')).toBeInTheDocument();
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

  it('offers Accessibility access when the inventory cannot be inspected', async () => {
    mockCommands({
      list_menu_bar_items: {
        supported: true,
        permissionGranted: false,
        dividerAvailable: false,
        items: [],
      } satisfies MenuBarInventory,
      request_accessibility: true,
    });
    renderView(<MenuBarView />);

    fireEvent.click(await screen.findByRole('button', { name: 'Grant Access…' }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('request_accessibility');
    });
  });

  it('offers the master switch when the feature is off', async () => {
    mockCommands({
      get_settings: { ...SETTINGS, menuBarTidyEnabled: false },
      get_menu_bar: { enabled: false, collapsed: true } satisfies MenuBarStatus,
    });
    renderView(<MenuBarView />);

    expect(await screen.findByText(/Menu bar tidying is off/)).toBeInTheDocument();

    fireEvent.click(screen.getByText('Turn On'));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('save_settings', {
        settings: expect.objectContaining({ menuBarTidyEnabled: true }),
      });
    });
  });
});
