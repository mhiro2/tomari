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
  theme: 'system',
  language: 'system',
  keyboardEnabled: true,
  windowManagementEnabled: true,
  externalWindowActionsEnabled: true,
  holdThresholdMs: 200,
  showInMenuBar: true,
  dragToSnapEnabled: false,
};

// WindowView reads settings (master switch, drag-to-snap) from the shared provider.
function renderView(ui: ReactElement) {
  return render(<SettingsProvider>{ui}</SettingsProvider>);
}

function mockCommands(overrides: Record<string, unknown> = {}) {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd in overrides) return Promise.resolve(overrides[cmd]);
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
});
