import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { ReactElement } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { SettingsProvider } from '../lib/settings';
import type { AppSettings } from '../lib/types';
import { WindowView } from './WindowView';

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
};

// WindowView reads settings (master switch, drag-to-snap) from the shared provider.
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
      case 'list_window_presets':
        return Promise.resolve(['leftHalf', 'maximize']);
      case 'accessibility_status':
        return Promise.resolve(true);
      case 'snap_window':
        return Promise.resolve('leftHalf');
      case 'get_settings':
        return Promise.resolve(SETTINGS);
      default:
        return Promise.resolve(null);
    }
  });
}

describe('WindowView', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockCommands();
  });

  it('renders presets and snaps the focused window on click', async () => {
    renderView(<WindowView />);

    const button = await screen.findByText('Left Half');
    fireEvent.click(button);

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('snap_window', { preset: 'leftHalf' });
    });
    expect(await screen.findByRole('status')).toHaveTextContent('Snapped to Left Half');
  });

  it('shows the cycled preset the backend actually applied', async () => {
    mockCommands({ snap_window: 'leftThird' });
    renderView(<WindowView />);

    fireEvent.click(await screen.findByText('Left Half'));
    expect(await screen.findByRole('status')).toHaveTextContent('Snapped to Left Third');
  });

  it('shows the permission banner when accessibility is not granted', async () => {
    mockCommands({ accessibility_status: false, list_window_presets: [] });

    renderView(<WindowView />);
    expect(await screen.findByText('Accessibility access needed')).toBeInTheDocument();
  });

  it('moves the window between displays and undoes the last move', async () => {
    renderView(<WindowView />);

    fireEvent.click(await screen.findByText('Next display →'));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('move_window_to_display', { direction: 'next' });
    });

    fireEvent.click(screen.getByText('↩ Undo last move'));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('undo_window');
    });
  });

  it('enables drag-to-snap and persists the toggle', async () => {
    renderView(<WindowView />);

    fireEvent.click(await screen.findByLabelText('Enable drag to snap'));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('save_settings', {
        settings: expect.objectContaining({ dragToSnapEnabled: true }),
      });
    });
  });

  it('shows an error instead of crashing when the initial preset/accessibility fetch fails', async () => {
    mockCommands({
      list_window_presets: Object.assign(new Error('presets unavailable'), { code: 'unknown' }),
      accessibility_status: Object.assign(new Error('status unavailable'), { code: 'unknown' }),
    });

    renderView(<WindowView />);

    // Both failures land on the same `status` output; whichever settles last wins,
    // so accept either message as evidence the rejection was caught, not thrown.
    const status = await screen.findByRole('status');
    expect(
      status.textContent === 'presets unavailable' || status.textContent === 'status unavailable',
    ).toBe(true);
  });

  it('shows an error instead of crashing when requesting accessibility access fails', async () => {
    mockCommands({
      accessibility_status: false,
      request_accessibility: Object.assign(new Error('grant failed'), { code: 'unknown' }),
    });

    renderView(<WindowView />);

    fireEvent.click(await screen.findByText('Grant access'));
    expect(await screen.findByRole('status')).toHaveTextContent('grant failed');
  });
});
