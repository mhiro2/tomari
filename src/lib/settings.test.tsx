import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { errorText } from './errors';
import { SettingsProvider, useSettings } from './settings';
import type { LiveApplyWarnings, AppSettings, SaveSettingsOutcome } from './types';

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
  menuBarTidyEnabled: false,
  menuBarAutoCollapseSecs: 0,
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

// Reports a clean rule-mutation outcome that speaks only for `capsLockRemap`.
function Reporter() {
  const { reportApplyOutcome } = useSettings();
  return (
    <button
      type="button"
      data-testid="report-clean"
      onClick={() => reportApplyOutcome({ applyWarnings: [] }, ['capsLockRemap'])}
    >
      report
    </button>
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

  it('seeds applyWarnings from the live state on load, before any save', async () => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_settings') return Promise.resolve(SETTINGS);
      if (cmd === 'get_apply_warnings')
        return Promise.resolve({ warnings: ['capsLockRemap'], unprobed: ['menuBar'] });
      return Promise.resolve(null);
    });

    render(
      <SettingsProvider>
        <StatusConsumer />
      </SettingsProvider>,
    );

    // A mismatch left over from an earlier session (a Caps Lock restore that
    // failed on quit) is shown as soon as the panel loads.
    await waitFor(() =>
      expect(screen.getByTestId('apply-warnings')).toHaveTextContent('capsLockRemap'),
    );
    expect(saveCalls()).toHaveLength(0);
  });

  it('re-reads the live applyWarnings each time the panel is shown', async () => {
    mockInvoke.mockReset();
    let liveWarnings: string[] = [];
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_settings') return Promise.resolve(SETTINGS);
      if (cmd === 'get_apply_warnings')
        return Promise.resolve({ warnings: liveWarnings, unprobed: ['menuBar'] });
      return Promise.resolve(null);
    });
    const handlers = new Map<string, () => void>();
    mockListen.mockImplementation((event: string, handler: () => void) => {
      handlers.set(event, handler);
      return Promise.resolve(() => {});
    });

    render(
      <SettingsProvider>
        <StatusConsumer />
      </SettingsProvider>,
    );
    await screen.findByTestId('toggle');
    await waitFor(() => expect(handlers.has('tomari:panel-shown')).toBe(true));
    expect(screen.getByTestId('apply-warnings')).toHaveTextContent('');

    // The panel was hidden, not destroyed; a mismatch arose meanwhile.
    liveWarnings = ['capsLockRemap'];
    await act(async () => {
      handlers.get('tomari:panel-shown')?.();
    });
    await waitFor(() =>
      expect(screen.getByTestId('apply-warnings')).toHaveTextContent('capsLockRemap'),
    );
  });

  it('does not let a slow live-warnings read overwrite a save outcome', async () => {
    mockInvoke.mockReset();
    let resolveLive: ((w: LiveApplyWarnings) => void) | null = null;
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_settings') return Promise.resolve(SETTINGS);
      if (cmd === 'get_apply_warnings')
        return new Promise<LiveApplyWarnings>((resolve) => {
          resolveLive = resolve;
        });
      if (cmd === 'save_settings') return Promise.resolve({ applyWarnings: ['launchAtLogin'] });
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

    // The live read that started at load resolves only now, with the stale
    // pre-save list: the fresher save outcome must stand.
    await act(async () => {
      resolveLive?.({ warnings: ['capsLockRemap'], unprobed: ['menuBar'] });
    });
    expect(screen.getByTestId('apply-warnings')).toHaveTextContent('launchAtLogin');
    expect(screen.getByTestId('apply-warnings')).not.toHaveTextContent('capsLockRemap');
  });

  it('lets only the newest of overlapping live-warning reads apply', async () => {
    mockInvoke.mockReset();
    const pending: Array<(w: LiveApplyWarnings) => void> = [];
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_settings') return Promise.resolve(SETTINGS);
      if (cmd === 'get_apply_warnings')
        return new Promise<LiveApplyWarnings>((resolve) => {
          pending.push(resolve);
        });
      return Promise.resolve(null);
    });
    const handlers = new Map<string, () => void>();
    mockListen.mockImplementation((event: string, handler: () => void) => {
      handlers.set(event, handler);
      return Promise.resolve(() => {});
    });

    render(
      <SettingsProvider>
        <StatusConsumer />
      </SettingsProvider>,
    );
    await screen.findByTestId('toggle');
    await waitFor(() => expect(pending).toHaveLength(1));
    // The panel is shown again before the load-time read has answered.
    await act(async () => {
      handlers.get('tomari:panel-shown')?.();
    });
    await waitFor(() => expect(pending).toHaveLength(2));

    // The newer read answers first, then the older one arrives late with a
    // stale list: the newer result must stand.
    await act(async () => {
      pending[1]?.({ warnings: ['keyboardTap'], unprobed: [] });
    });
    await waitFor(() =>
      expect(screen.getByTestId('apply-warnings')).toHaveTextContent('keyboardTap'),
    );
    await act(async () => {
      pending[0]?.({ warnings: ['capsLockRemap'], unprobed: [] });
    });
    expect(screen.getByTestId('apply-warnings')).toHaveTextContent('keyboardTap');
    expect(screen.getByTestId('apply-warnings')).not.toHaveTextContent('capsLockRemap');
  });

  it('keeps an unprobed warning from the last save when the panel is shown again', async () => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_settings') return Promise.resolve(SETTINGS);
      if (cmd === 'get_apply_warnings')
        return Promise.resolve({ warnings: ['capsLockRemap'], unprobed: ['menuBar'] });
      if (cmd === 'save_settings')
        return Promise.resolve({ applyWarnings: ['menuBar', 'launchAtLogin'] });
      return Promise.resolve(null);
    });
    const handlers = new Map<string, () => void>();
    mockListen.mockImplementation((event: string, handler: () => void) => {
      handlers.set(event, handler);
      return Promise.resolve(() => {});
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
      expect(screen.getByTestId('apply-warnings')).toHaveTextContent('menuBar,launchAtLogin'),
    );

    await act(async () => {
      handlers.get('tomari:panel-shown')?.();
    });
    // `menuBar` has no live probe, so the save's verdict stands; the probed
    // codes are replaced by what the live state says now.
    await waitFor(() =>
      expect(screen.getByTestId('apply-warnings')).toHaveTextContent('capsLockRemap,menuBar'),
    );
    expect(screen.getByTestId('apply-warnings')).not.toHaveTextContent('launchAtLogin');
  });

  it('merges a reported outcome into the shared warnings, replacing only the probed codes', async () => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_settings') return Promise.resolve(SETTINGS);
      if (cmd === 'save_settings')
        return Promise.resolve({ applyWarnings: ['menuBar', 'capsLockRemap'] });
      return Promise.resolve(null);
    });
    render(
      <SettingsProvider>
        <StatusConsumer />
        <Reporter />
      </SettingsProvider>,
    );
    const btn = await screen.findByTestId('toggle');
    await act(async () => {
      fireEvent.click(btn);
    });
    await waitFor(() =>
      expect(screen.getByTestId('apply-warnings')).toHaveTextContent('menuBar,capsLockRemap'),
    );

    // A rule save that applied cleanly speaks only for `capsLockRemap`: that
    // code clears, `menuBar` — outside what it probed — keeps its verdict.
    await act(async () => {
      fireEvent.click(screen.getByTestId('report-clean'));
    });
    expect(screen.getByTestId('apply-warnings')).toHaveTextContent('menuBar');
    expect(screen.getByTestId('apply-warnings')).not.toHaveTextContent('capsLockRemap');
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
