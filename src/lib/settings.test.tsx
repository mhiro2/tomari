import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { errorText } from './errors';
import { SettingsProvider, useSettings } from './settings';
import type {
  AppSettings,
  ConfigurationWarnings,
  LiveApplyWarnings,
  SaveSettingsOutcome,
} from './types';

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
  const {
    settings,
    settingsRecovery,
    retrySettingsRecovery,
    resetSettingsRecovery,
    loadError,
    retryLoad,
    saveError,
    applyWarnings,
    configurationWarnings,
    update,
  } = useSettings();
  return (
    <div>
      {settingsRecovery !== null && (
        <div>
          <span data-testid="recovery-kind">{settingsRecovery.kind}</span>
          <span data-testid="recovery-phase">{settingsRecovery.phase}</span>
          {settingsRecovery.phase === 'failed' && (
            <span data-testid="recovery-error">{errorText(settingsRecovery.error)}</span>
          )}
          <button type="button" onClick={() => void retrySettingsRecovery()}>
            retry recovery
          </button>
          <button type="button" onClick={() => void resetSettingsRecovery()}>
            reset recovery
          </button>
        </div>
      )}
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
      <span data-testid="configuration-warnings-revision">
        {configurationWarnings?.revision ?? 'none'}
      </span>
      <span data-testid="configuration-warning-ids">
        {[
          ...(configurationWarnings?.invalidHotkeys ?? []),
          ...(configurationWarnings?.invalidModifierRules ?? []),
        ]
          .map((issue) => issue.id)
          .join(',')}
      </span>
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

  it('installs the configuration-warning listener before pulling its snapshot', async () => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_settings') return Promise.resolve(SETTINGS);
      if (cmd === 'get_configuration_warnings') {
        return Promise.resolve({ invalidHotkeys: [], invalidModifierRules: [], revision: 0 });
      }
      return Promise.resolve(null);
    });
    let finishListener: (() => void) | undefined;
    mockListen.mockImplementation((event: string) => {
      if (event === 'tomari:configuration-warnings-changed') {
        return new Promise<() => void>((resolve) => {
          finishListener = () => resolve(() => {});
        });
      }
      return Promise.resolve(() => {});
    });

    render(
      <SettingsProvider>
        <StatusConsumer />
      </SettingsProvider>,
    );

    await screen.findByTestId('toggle');
    await waitFor(() => expect(finishListener).toBeDefined());
    expect(mockInvoke.mock.calls.some(([cmd]) => cmd === 'get_configuration_warnings')).toBe(false);

    await act(async () => finishListener?.());
    await waitFor(() =>
      expect(mockInvoke.mock.calls.some(([cmd]) => cmd === 'get_configuration_warnings')).toBe(
        true,
      ),
    );
  });

  it('keeps an event snapshot when the pull returns the same or an older revision', async () => {
    mockInvoke.mockReset();
    let resolvePull: ((warnings: ConfigurationWarnings) => void) | undefined;
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_settings') return Promise.resolve(SETTINGS);
      if (cmd === 'get_configuration_warnings') {
        return new Promise<ConfigurationWarnings>((resolve) => {
          resolvePull = resolve;
        });
      }
      return Promise.resolve(null);
    });
    let configurationChanged: ((event: { payload: ConfigurationWarnings }) => void) | undefined;
    mockListen.mockImplementation(
      (event: string, handler: (event: { payload: ConfigurationWarnings }) => void) => {
        if (event === 'tomari:configuration-warnings-changed') configurationChanged = handler;
        return Promise.resolve(() => {});
      },
    );

    render(
      <SettingsProvider>
        <StatusConsumer />
      </SettingsProvider>,
    );
    await waitFor(() => expect(resolvePull).toBeDefined());

    const fromEvent: ConfigurationWarnings = {
      invalidHotkeys: [
        { id: 'event-winner', label: 'Event winner', reason: 'unsafeGlobalShortcut' },
      ],
      invalidModifierRules: [],
      revision: 4,
    };
    await act(async () => configurationChanged?.({ payload: fromEvent }));
    expect(screen.getByTestId('configuration-warning-ids')).toHaveTextContent('event-winner');

    await act(async () => {
      resolvePull?.({
        invalidHotkeys: [{ id: 'equal-pull', label: 'Equal pull', reason: 'invalidAccelerator' }],
        invalidModifierRules: [],
        revision: 4,
      });
    });
    expect(screen.getByTestId('configuration-warning-ids')).toHaveTextContent('event-winner');
    expect(screen.getByTestId('configuration-warning-ids')).not.toHaveTextContent('equal-pull');

    await act(async () =>
      configurationChanged?.({
        payload: {
          invalidHotkeys: [],
          invalidModifierRules: [
            { id: 'older-event', label: 'Older event', reason: 'hyperWithRemap' },
          ],
          revision: 3,
        },
      }),
    );
    expect(screen.getByTestId('configuration-warning-ids')).toHaveTextContent('event-winner');
    expect(screen.getByTestId('configuration-warnings-revision')).toHaveTextContent('4');
  });

  it('still pulls configuration warnings when listener registration fails', async () => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_settings') return Promise.resolve(SETTINGS);
      if (cmd === 'get_configuration_warnings') {
        return Promise.resolve({
          invalidHotkeys: [],
          invalidModifierRules: [
            { id: 'legacy-caps', label: 'Caps Lock', reason: 'hyperWithRemap' },
          ],
          revision: 7,
        });
      }
      return Promise.resolve(null);
    });
    mockListen.mockImplementation((event: string) =>
      event === 'tomari:configuration-warnings-changed'
        ? Promise.reject(new Error('event bridge unavailable'))
        : Promise.resolve(() => {}),
    );

    render(
      <SettingsProvider>
        <StatusConsumer />
      </SettingsProvider>,
    );

    await waitFor(() =>
      expect(screen.getByTestId('configuration-warning-ids')).toHaveTextContent('legacy-caps'),
    );
    expect(screen.getByTestId('configuration-warnings-revision')).toHaveTextContent('7');
  });

  it('retries a failed configuration-warning pull when the panel is shown again', async () => {
    mockInvoke.mockReset();
    let warningReads = 0;
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_settings') return Promise.resolve(SETTINGS);
      if (cmd === 'get_configuration_warnings') {
        warningReads += 1;
        return warningReads === 1
          ? Promise.reject(new Error('temporary bridge failure'))
          : Promise.resolve({
              invalidHotkeys: [
                { id: 'recovered-hotkey', label: 'Recovered', reason: 'invalidAccelerator' },
              ],
              invalidModifierRules: [],
              revision: 3,
            });
      }
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
    await waitFor(() => expect(warningReads).toBe(1));
    expect(screen.getByTestId('configuration-warnings-revision')).toHaveTextContent('none');

    await act(async () => handlers.get('tomari:panel-shown')?.());

    await waitFor(() =>
      expect(screen.getByTestId('configuration-warning-ids')).toHaveTextContent('recovered-hotkey'),
    );
    expect(warningReads).toBe(2);
  });

  it('clears stale configuration warnings on a panel pull when live events are unavailable', async () => {
    mockInvoke.mockReset();
    let warningReads = 0;
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_settings') return Promise.resolve(SETTINGS);
      if (cmd === 'get_configuration_warnings') {
        warningReads += 1;
        return Promise.resolve(
          warningReads === 1
            ? {
                invalidHotkeys: [
                  { id: 'stale-hotkey', label: 'Stale', reason: 'unsafeGlobalShortcut' },
                ],
                invalidModifierRules: [],
                revision: 8,
              }
            : { invalidHotkeys: [], invalidModifierRules: [], revision: 9 },
        );
      }
      return Promise.resolve(null);
    });
    const handlers = new Map<string, () => void>();
    mockListen.mockImplementation((event: string, handler: () => void) => {
      if (event === 'tomari:configuration-warnings-changed') {
        return Promise.reject(new Error('event bridge unavailable'));
      }
      handlers.set(event, handler);
      return Promise.resolve(() => {});
    });

    render(
      <SettingsProvider>
        <StatusConsumer />
      </SettingsProvider>,
    );

    await waitFor(() =>
      expect(screen.getByTestId('configuration-warning-ids')).toHaveTextContent('stale-hotkey'),
    );
    await act(async () => handlers.get('tomari:panel-shown')?.());

    await waitFor(() =>
      expect(screen.getByTestId('configuration-warning-ids')).toBeEmptyDOMElement(),
    );
    expect(screen.getByTestId('configuration-warnings-revision')).toHaveTextContent('9');
  });

  it('drops an older configuration-warning pull that resolves after a panel refresh', async () => {
    mockInvoke.mockReset();
    const pending: Array<(warnings: ConfigurationWarnings) => void> = [];
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_settings') return Promise.resolve(SETTINGS);
      if (cmd === 'get_configuration_warnings') {
        return new Promise<ConfigurationWarnings>((resolve) => pending.push(resolve));
      }
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

    await waitFor(() => expect(pending).toHaveLength(1));
    await act(async () => handlers.get('tomari:panel-shown')?.());
    await waitFor(() => expect(pending).toHaveLength(2));

    await act(async () => {
      pending[1]?.({ invalidHotkeys: [], invalidModifierRules: [], revision: 6 });
    });
    expect(screen.getByTestId('configuration-warnings-revision')).toHaveTextContent('6');

    await act(async () => {
      pending[0]?.({
        invalidHotkeys: [
          { id: 'late-stale-hotkey', label: 'Late stale', reason: 'invalidAccelerator' },
        ],
        invalidModifierRules: [],
        revision: 5,
      });
    });
    expect(screen.getByTestId('configuration-warning-ids')).not.toHaveTextContent(
      'late-stale-hotkey',
    );
    expect(screen.getByTestId('configuration-warnings-revision')).toHaveTextContent('6');
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

  it('separates settings recovery from a generic load error and skips live warning reads', async () => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_settings') {
        return Promise.reject({
          code: 'settingsRecoveryRequired',
          message: 'settings row does not decode',
        });
      }
      return Promise.resolve(null);
    });

    render(
      <SettingsProvider>
        <StatusConsumer />
      </SettingsProvider>,
    );

    expect(await screen.findByTestId('recovery-phase')).toHaveTextContent('required');
    expect(screen.getByTestId('recovery-kind')).toHaveTextContent('retryable');
    expect(screen.queryByTestId('load-error')).not.toBeInTheDocument();
    expect(screen.queryByTestId('toggle')).not.toBeInTheDocument();
    expect(mockInvoke.mock.calls.some(([cmd]) => cmd === 'get_apply_warnings')).toBe(false);
    expect(mockInvoke.mock.calls.some(([cmd]) => cmd === 'get_configuration_warnings')).toBe(false);
    expect(
      mockListen.mock.calls.some(([event]) => event === 'tomari:configuration-warnings-changed'),
    ).toBe(false);
  });

  it('drops configuration warnings and ignores late events after recovery becomes required', async () => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_settings') return Promise.resolve(SETTINGS);
      if (cmd === 'get_configuration_warnings') {
        return Promise.resolve({
          invalidHotkeys: [
            { id: 'legacy-hotkey', label: 'Legacy hotkey', reason: 'invalidAccelerator' },
          ],
          invalidModifierRules: [],
          revision: 1,
        });
      }
      if (cmd === 'save_settings') {
        return Promise.reject({
          code: 'settingsRecoveryRequired',
          message: 'settings row no longer decodes',
        });
      }
      return Promise.resolve(null);
    });
    let configurationChanged: ((event: { payload: ConfigurationWarnings }) => void) | undefined;
    mockListen.mockImplementation(
      (event: string, handler: (event: { payload: ConfigurationWarnings }) => void) => {
        if (event === 'tomari:configuration-warnings-changed') configurationChanged = handler;
        return Promise.resolve(() => {});
      },
    );

    render(
      <SettingsProvider>
        <StatusConsumer />
      </SettingsProvider>,
    );
    await waitFor(() =>
      expect(screen.getByTestId('configuration-warning-ids')).toHaveTextContent('legacy-hotkey'),
    );

    fireEvent.click(screen.getByTestId('toggle'));
    expect(await screen.findByTestId('recovery-phase')).toHaveTextContent('required');
    expect(screen.getByTestId('configuration-warning-ids')).toHaveTextContent('');

    await act(async () =>
      configurationChanged?.({
        payload: {
          invalidHotkeys: [],
          invalidModifierRules: [
            { id: 'late-event', label: 'Late event', reason: 'duplicateModifierSlot' },
          ],
          revision: 2,
        },
      }),
    );
    expect(screen.getByTestId('configuration-warning-ids')).toHaveTextContent('');
  });

  it('marks a quarantined database as reset-only and refuses a retry dispatch', async () => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_settings') {
        return Promise.reject({
          code: 'databaseResetRequired',
          message: 'settings database was quarantined',
        });
      }
      return Promise.resolve(null);
    });

    render(
      <SettingsProvider>
        <StatusConsumer />
      </SettingsProvider>,
    );

    expect(await screen.findByTestId('recovery-kind')).toHaveTextContent('databaseReset');
    fireEvent.click(screen.getByRole('button', { name: 'retry recovery' }));
    expect(mockInvoke.mock.calls.some(([cmd]) => cmd === 'retry_settings_recovery')).toBe(false);
    expect(screen.getByTestId('recovery-phase')).toHaveTextContent('required');
  });

  it('ignores a settings-changed event until an explicit recovery action succeeds', async () => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_settings') {
        return Promise.reject({
          code: 'settingsRecoveryRequired',
          message: 'settings row does not decode',
        });
      }
      return Promise.resolve(null);
    });
    let settingsChanged: ((event: { payload: AppSettings }) => void) | undefined;
    mockListen.mockImplementation(
      (event: string, handler: (event: { payload: AppSettings }) => void) => {
        if (event === 'tomari:settings-changed') settingsChanged = handler;
        return Promise.resolve(() => {});
      },
    );

    render(
      <SettingsProvider>
        <StatusConsumer />
      </SettingsProvider>,
    );
    await screen.findByTestId('recovery-phase');

    await act(async () => {
      settingsChanged?.({ payload: SETTINGS });
    });

    expect(screen.getByTestId('recovery-phase')).toHaveTextContent('required');
    expect(screen.queryByTestId('toggle')).not.toBeInTheDocument();
  });

  it('keeps recovery active and exposes a local retry error when retry rejects', async () => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_settings') {
        return Promise.reject({
          code: 'settingsRecoveryRequired',
          message: 'settings row does not decode',
        });
      }
      if (cmd === 'retry_settings_recovery') return Promise.reject(new Error('still unreadable'));
      return Promise.resolve(null);
    });

    render(
      <SettingsProvider>
        <StatusConsumer />
      </SettingsProvider>,
    );
    await screen.findByTestId('recovery-phase');
    fireEvent.click(screen.getByRole('button', { name: 'retry recovery' }));

    expect(await screen.findByTestId('recovery-phase')).toHaveTextContent('failed');
    expect(screen.getByTestId('recovery-error')).toHaveTextContent('still unreadable');
    expect(screen.queryByTestId('toggle')).not.toBeInTheDocument();
  });

  it('re-loads settings when a mocked retry command resolves', async () => {
    mockInvoke.mockReset();
    let reads = 0;
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_settings') {
        reads += 1;
        return reads === 1
          ? Promise.reject({
              code: 'settingsRecoveryRequired',
              message: 'settings row does not decode',
            })
          : Promise.resolve(SETTINGS);
      }
      if (cmd === 'retry_settings_recovery') return Promise.resolve();
      if (cmd === 'get_apply_warnings') return Promise.resolve({ warnings: [], unprobed: [] });
      return Promise.resolve(null);
    });

    render(
      <SettingsProvider>
        <StatusConsumer />
      </SettingsProvider>,
    );
    await screen.findByTestId('recovery-phase');
    fireEvent.click(screen.getByRole('button', { name: 'retry recovery' }));

    expect(await screen.findByTestId('toggle')).toBeInTheDocument();
    expect(screen.queryByTestId('recovery-phase')).not.toBeInTheDocument();
    expect(reads).toBe(2);
  });
});
