import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { errorText } from './errors';
import { SettingsProvider, useSettings } from './settings';
import type { AppSettings, SaveSettingsOutcome } from './types';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
const { invoke } = await import('@tauri-apps/api/core');
const mockInvoke = vi.mocked(invoke);

const { mockListen } = vi.hoisted(() => ({ mockListen: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: mockListen,
  emit: vi.fn(() => Promise.resolve()),
}));

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

// A view of the load/save error state, so tests can assert on it without
// depending on i18n or App.tsx (which this task must not touch).
function StatusConsumer() {
  const { settings, loadError, retryLoad, saveError, applyWarnings, update } = useSettings();
  return (
    <div>
      {loadError !== null && (
        <div>
          <span data-testid="load-error">{errorText(loadError)}</span>
          <button type="button" onClick={retryLoad}>
            retry
          </button>
        </div>
      )}
      {saveError !== null && <span data-testid="save-error">{errorText(saveError)}</span>}
      <span data-testid="apply-warnings">{applyWarnings.join(',')}</span>
      {settings && (
        <button
          type="button"
          onClick={() => update({ launchAtLogin: !settings.launchAtLogin })}
          data-testid="toggle"
        >
          {String(settings.launchAtLogin)}
        </button>
      )}
    </div>
  );
}

describe('SettingsProvider', () => {
  beforeEach(() => {
    // Default: no-op listener, matching real usage where events rarely fire.
    mockListen.mockReset();
    mockListen.mockImplementation(() => Promise.resolve(() => {}));
  });

  it('optimistically updates the UI and persists the new value', async () => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_settings') return Promise.resolve(SETTINGS);
      if (cmd === 'save_settings') return Promise.resolve({ applyWarnings: [] });
      return Promise.resolve(null);
    });

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
      if (cmd === 'save_settings')
        return new Promise<SaveSettingsOutcome>((resolve) =>
          resolvers.push(() => resolve({ applyWarnings: [] })),
        );
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

  it('re-syncs from disk and keeps saveError on a save failure', async () => {
    mockInvoke.mockReset();
    const fresh: AppSettings = { ...SETTINGS, launchAtLogin: true, showInMenuBar: false };
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_settings') {
        // First call: initial load. Second call: post-failure re-sync.
        const priorCalls = mockInvoke.mock.calls.filter(([c]) => c === 'get_settings').length;
        return Promise.resolve(priorCalls === 1 ? SETTINGS : fresh);
      }
      if (cmd === 'save_settings') return Promise.reject(new Error('disk full'));
      return Promise.resolve(null);
    });

    render(
      <SettingsProvider>
        <StatusConsumer />
      </SettingsProvider>,
    );

    const btn = await screen.findByTestId('toggle');
    await act(async () => {
      fireEvent.click(btn);
    });

    // The failed save triggers a re-sync; the UI reflects what's actually on disk.
    await waitFor(() => expect(screen.getByTestId('save-error')).toHaveTextContent('disk full'));
    expect(btn).toHaveTextContent('true');
    expect(mockInvoke.mock.calls.filter(([c]) => c === 'get_settings')).toHaveLength(2);
  });

  it('does not clobber a newer edit with the post-failure re-sync when dirty', async () => {
    mockInvoke.mockReset();
    let saveCallCount = 0;
    const resyncResolvers: ((v: AppSettings) => void)[] = [];
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_settings') {
        return new Promise<AppSettings>((resolve) => resyncResolvers.push(resolve));
      }
      if (cmd === 'save_settings') {
        saveCallCount += 1;
        if (saveCallCount === 1) return Promise.reject(new Error('boom'));
        return Promise.resolve({ applyWarnings: [] });
      }
      return Promise.resolve(null);
    });

    render(
      <SettingsProvider>
        <StatusConsumer />
      </SettingsProvider>,
    );

    // Initial load resolves via the first get_settings call.
    await act(async () => {
      resyncResolvers.shift()?.(SETTINGS);
    });

    const btn = await screen.findByTestId('toggle');

    // Edit -> save rejects -> re-sync (get_settings) starts.
    await act(async () => {
      fireEvent.click(btn);
    });
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('save_settings', expect.anything()),
    );

    // A second edit arrives while the re-sync is still in flight: this sets
    // `dirty`, so the re-sync result below must not overwrite it.
    await act(async () => {
      fireEvent.click(btn);
    });

    // Resolve the re-sync with a stale snapshot; it must be discarded because
    // `dirty` is set.
    await act(async () => {
      resyncResolvers.shift()?.({ ...SETTINGS, launchAtLogin: true, showInMenuBar: false });
    });

    // The second edit (back to false) must win, and its own save must run.
    await waitFor(() => expect(saveCallCount).toBe(2));
    expect(btn).toHaveTextContent('false');
  });

  it('keeps the last applyWarnings when a later save fails', async () => {
    mockInvoke.mockReset();
    let saveCallCount = 0;
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_settings') return Promise.resolve(SETTINGS);
      if (cmd === 'save_settings') {
        saveCallCount += 1;
        if (saveCallCount === 1) {
          return Promise.resolve({ applyWarnings: ['launchAtLogin'] });
        }
        return Promise.reject(new Error('write failed'));
      }
      return Promise.resolve(null);
    });

    render(
      <SettingsProvider>
        <StatusConsumer />
      </SettingsProvider>,
    );

    const btn = await screen.findByTestId('toggle');
    await act(async () => {
      fireEvent.click(btn);
    });
    await waitFor(() =>
      expect(screen.getByTestId('apply-warnings')).toHaveTextContent('launchAtLogin'),
    );

    await act(async () => {
      fireEvent.click(btn);
    });
    await waitFor(() => expect(screen.getByTestId('save-error')).toHaveTextContent('write failed'));
    // The second save's failure must not clear the still-relevant warning from
    // the first (successful) save.
    expect(screen.getByTestId('apply-warnings')).toHaveTextContent('launchAtLogin');
  });

  it('surfaces an initial load failure and recovers via retryLoad', async () => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_settings') return Promise.reject(new Error('offline'));
      return Promise.resolve(null);
    });

    render(
      <SettingsProvider>
        <StatusConsumer />
      </SettingsProvider>,
    );

    await waitFor(() => expect(screen.getByTestId('load-error')).toHaveTextContent('offline'));
    expect(screen.queryByTestId('toggle')).not.toBeInTheDocument();

    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_settings') return Promise.resolve(SETTINGS);
      return Promise.resolve(null);
    });

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'retry' }));
    });

    await waitFor(() => expect(screen.getByTestId('toggle')).toBeInTheDocument());
    expect(screen.queryByTestId('load-error')).not.toBeInTheDocument();
  });

  it('clears a stale loadError once a settings-changed event lands', async () => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_settings') return Promise.reject(new Error('offline'));
      return Promise.resolve(null);
    });

    let eventHandler: ((e: { payload: AppSettings }) => void) | undefined;
    mockListen.mockImplementation(
      (_event: string, handler: (e: { payload: AppSettings }) => void) => {
        eventHandler = handler;
        return Promise.resolve(() => {});
      },
    );

    render(
      <SettingsProvider>
        <StatusConsumer />
      </SettingsProvider>,
    );

    // The initial load fails, so the error + retry UI is showing.
    await waitFor(() => expect(screen.getByTestId('load-error')).toHaveTextContent('offline'));

    // A broadcast arrives anyway (e.g. another window saved successfully) and
    // must clear the stale loadError — settings and loadError must never both
    // be set at once.
    await act(async () => {
      eventHandler?.({ payload: SETTINGS });
    });

    expect(screen.queryByTestId('load-error')).not.toBeInTheDocument();
    expect(screen.getByTestId('toggle')).toBeInTheDocument();
  });

  it('discards a slow initial load once a settings-changed event has been applied', async () => {
    mockInvoke.mockReset();
    let resolveInitialLoad: ((s: AppSettings) => void) | undefined;
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_settings') {
        return new Promise<AppSettings>((resolve) => {
          resolveInitialLoad = resolve;
        });
      }
      return Promise.resolve(null);
    });

    let eventHandler: ((e: { payload: AppSettings }) => void) | undefined;
    mockListen.mockImplementation(
      (_event: string, handler: (e: { payload: AppSettings }) => void) => {
        eventHandler = handler;
        return Promise.resolve(() => {});
      },
    );

    render(
      <SettingsProvider>
        <StatusConsumer />
      </SettingsProvider>,
    );

    await waitFor(() => expect(eventHandler).toBeDefined());

    // The broadcast arrives and is applied before the initial load resolves.
    const fromEvent: AppSettings = { ...SETTINGS, launchAtLogin: true };
    await act(async () => {
      eventHandler?.({ payload: fromEvent });
    });
    expect(screen.getByTestId('toggle')).toHaveTextContent('true');

    // The slow initial load now resolves with the original (stale) snapshot;
    // it must not clobber the settings-changed value already applied.
    await act(async () => {
      resolveInitialLoad?.(SETTINGS);
    });
    expect(screen.getByTestId('toggle')).toHaveTextContent('true');
  });
});
