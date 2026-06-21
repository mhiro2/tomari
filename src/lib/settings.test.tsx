import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { SettingsProvider, useSettings } from './settings';
import type { AppSettings } from './types';

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

// A button that flips "launch at login" and shows the current value.
function Consumer() {
  const { settings, update } = useSettings();
  if (!settings) return null;
  return (
    <button type="button" onClick={() => update({ launchAtLogin: !settings.launchAtLogin })}>
      {String(settings.launchAtLogin)}
    </button>
  );
}

function saveCalls() {
  return mockInvoke.mock.calls.filter(([cmd]) => cmd === 'save_settings');
}

describe('SettingsProvider', () => {
  it('optimistically updates the UI and persists the new value', async () => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation((cmd: string) =>
      cmd === 'get_settings' ? Promise.resolve(SETTINGS) : Promise.resolve(null),
    );

    render(
      <SettingsProvider>
        <Consumer />
      </SettingsProvider>,
    );

    const btn = await screen.findByRole('button');
    await act(async () => {
      fireEvent.click(btn);
    });

    expect(btn).toHaveTextContent('true');
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('save_settings', {
        settings: expect.objectContaining({ launchAtLogin: true }),
      }),
    );
  });

  it('serializes saves and the last write wins when edits overlap', async () => {
    mockInvoke.mockReset();
    const resolvers: (() => void)[] = [];
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_settings') return Promise.resolve(SETTINGS);
      if (cmd === 'save_settings') return new Promise<void>((resolve) => resolvers.push(resolve));
      return Promise.resolve(null);
    });

    render(
      <SettingsProvider>
        <Consumer />
      </SettingsProvider>,
    );

    const btn = await screen.findByRole('button');

    // First edit starts a save that stays in flight.
    await act(async () => {
      fireEvent.click(btn);
    });
    expect(saveCalls()).toHaveLength(1);
    expect(saveCalls()[0]?.[1]).toEqual({
      settings: expect.objectContaining({ launchAtLogin: true }),
    });

    // Second edit while the first save is pending must not start a second write.
    await act(async () => {
      fireEvent.click(btn);
    });
    expect(saveCalls()).toHaveLength(1);

    // Let the first save finish: the saver re-runs once with the latest value.
    await act(async () => {
      resolvers[0]?.();
    });
    await waitFor(() => expect(saveCalls()).toHaveLength(2));
    expect(saveCalls()[1]?.[1]).toEqual({
      settings: expect.objectContaining({ launchAtLogin: false }),
    });

    await act(async () => {
      resolvers[1]?.();
    });
  });
});
