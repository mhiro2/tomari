import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { SettingsProvider, useSettings } from './settings';
import type { AppSettings } from './types';

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

// A button that bumps the hold threshold by 10 and shows the current value.
function Consumer() {
  const { settings, update } = useSettings();
  if (!settings) return null;
  return (
    <button
      type="button"
      onClick={() => update({ holdThresholdMs: settings.holdThresholdMs + 10 })}
    >
      {settings.holdThresholdMs}
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

    expect(btn).toHaveTextContent('210');
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('save_settings', {
        settings: expect.objectContaining({ holdThresholdMs: 210 }),
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
      settings: expect.objectContaining({ holdThresholdMs: 210 }),
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
      settings: expect.objectContaining({ holdThresholdMs: 220 }),
    });

    await act(async () => {
      resolvers[1]?.();
    });
  });
});
