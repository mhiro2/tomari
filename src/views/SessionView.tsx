import { listen } from '@tauri-apps/api/event';
import { useEffect, useEffectEvent, useRef, useState } from 'react';

import { Chip, FeaturePageHeader, SettingsList, SettingsRow, Toggle } from '../components/ui';
import * as api from '../lib/api';
import { formatCmdError } from '../lib/errors';
import { useT, type MessageKey, type Translator } from '../lib/i18n';
import type {
  KeepAwakeNotice,
  KeepAwakeOptions,
  KeepAwakePhase,
  KeepAwakeStatus,
  LidCloseState,
  PowerSource,
} from '../lib/types';

type TimerMode = 'never' | '30m' | '1h' | 'time';

const padTimePart = (value: number) => String(value).padStart(2, '0');

const DEFAULT_OPTIONS: KeepAwakeOptions = {
  durationSecs: null,
  endsAtMs: null,
  acOnly: false,
  lowBatteryAction: 'warn',
};

const LID_CLOSE_CHIP: Record<
  LidCloseState,
  { tone: 'ok' | 'warn' | 'err' | 'muted'; key: MessageKey }
> = {
  engaged: { tone: 'ok', key: 'settings.lidActive' },
  pending: { tone: 'warn', key: 'settings.lidPending' },
  unavailable: { tone: 'err', key: 'settings.lidUnavailable' },
  off: { tone: 'muted', key: 'settings.lidOff' },
};

const PHASE_LABEL: Record<KeepAwakePhase, MessageKey> = {
  off: 'settings.keepAwakeInactive',
  enabling: 'settings.keepAwakeEnabling',
  on: 'settings.keepAwakeActive',
  disabling: 'settings.keepAwakeDisabling',
  failed: 'settings.keepAwakeFailed',
};

const NOTICE_LABEL: Record<KeepAwakeNotice, MessageKey> = {
  acRequired: 'settings.noticeAcRequired',
  acDisconnected: 'settings.noticeAcDisconnected',
  lowBattery: 'settings.noticeLowBattery',
  timerElapsed: 'settings.noticeTimerElapsed',
  authorizationDeclined: 'settings.noticeAuthorizationDeclined',
  lidCloseUnconfirmed: 'settings.noticeLidCloseUnconfirmed',
};

const POWER_LABEL: Record<PowerSource, MessageKey> = {
  ac: 'settings.powerAc',
  battery: 'settings.powerBattery',
  unknown: 'settings.systemUnknown',
};

function deadlineFor(mode: TimerMode, customTime: string): number | null {
  if (mode === 'time') {
    const value = new Date(customTime).getTime();
    return Number.isFinite(value) ? value : null;
  }
  return null;
}

function timerFields(
  mode: TimerMode,
  customTime: string,
): Pick<KeepAwakeOptions, 'durationSecs' | 'endsAtMs'> {
  if (mode === '30m') return { durationSecs: 30 * 60, endsAtMs: null };
  if (mode === '1h') return { durationSecs: 60 * 60, endsAtMs: null };
  return { durationSecs: null, endsAtMs: deadlineFor(mode, customTime) };
}

function localDateTimeValue(timestamp: number): string {
  const date = new Date(timestamp);
  return `${date.getFullYear()}-${padTimePart(date.getMonth() + 1)}-${padTimePart(
    date.getDate(),
  )}T${padTimePart(date.getHours())}:${padTimePart(date.getMinutes())}`;
}

function formatCountdown(deadline: number, now: number): string {
  const total = Math.max(0, Math.ceil((deadline - now) / 1_000));
  const hours = Math.floor(total / 3_600);
  const minutes = Math.floor((total % 3_600) / 60);
  const seconds = total % 60;
  return hours > 0
    ? `${hours}:${String(minutes).padStart(2, '0')}:${String(seconds).padStart(2, '0')}`
    : `${minutes}:${String(seconds).padStart(2, '0')}`;
}

function formatElapsed(seconds: number): string {
  const hours = Math.floor(seconds / 3_600);
  const minutes = Math.floor((seconds % 3_600) / 60);
  return hours > 0 ? `${hours}h ${minutes}m` : `${minutes}m`;
}

// Keep-awake is runtime-only state (it never persists), so it lives outside
// AppSettings. The backend owns every transition and safety deadline; this view
// renders that state machine and follows its event while the panel is open.
export function SessionView() {
  const t = useT();
  const [status, setStatus] = useState<KeepAwakeStatus | null>(null);
  const [commandBusy, setCommandBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [timerMode, setTimerMode] = useState<TimerMode>('never');
  const [customTime, setCustomTime] = useState('');
  const [draftOptions, setDraftOptions] = useState(DEFAULT_OPTIONS);
  const [now, setNow] = useState(Date.now());
  // Highest revision applied so far. Several backend threads emit, each
  // snapshotting before it emits, so an older snapshot can still arrive last.
  const appliedRevision = useRef(-1);

  const reportLoadError = useEffectEvent((e: unknown) => setError(formatCmdError(e, t)));

  // The single writer of every rendered field. The backend owns the options, so
  // mirror whatever it reports back into the timer controls — that matters for
  // the deadlines it drops: an end time already in the past is spent, and
  // engaging clears it, so without this the select would keep claiming a
  // deadline that is no longer enforced.
  const applyStatus = useEffectEvent((next: KeepAwakeStatus) => {
    // Rendering an older snapshot would strand the panel on a transition that
    // has already finished — with every toggle disabled and no further event
    // coming to release it.
    if (next.revision < appliedRevision.current) return;
    appliedRevision.current = next.revision;
    setStatus(next);
    const options = next.options;
    setDraftOptions(options);
    if (options.durationSecs === 30 * 60) {
      setTimerMode('30m');
    } else if (options.durationSecs === 60 * 60) {
      setTimerMode('1h');
    } else if (options.endsAtMs !== null) {
      setTimerMode('time');
      setCustomTime(localDateTimeValue(options.endsAtMs));
    } else if (timerMode === 'time' && customTime !== '') {
      // We had sent an end time and it came back gone: the backend dropped it.
      setTimerMode('never');
      setCustomTime('');
    } else if (timerMode !== 'time') {
      // 'time' with an empty field is the user picking "At a time…" before
      // typing one, so only the other modes fall back to never.
      setTimerMode('never');
    }
  });

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    // Subscribe *before* the first read. An event emitted in the gap between the
    // two would be lost outright, and no revision check can recover a snapshot
    // that never arrived — a transition settling in that window would leave the
    // panel stuck on the pending phase the read returned.
    async function start() {
      try {
        const stop = await listen<KeepAwakeStatus>('tomari:keep-awake-changed', (event) =>
          applyStatus(event.payload),
        );
        if (cancelled) {
          stop();
          return;
        }
        unlisten = stop;
      } catch {
        // Without the subscription the panel is static, but a read still gives
        // it something truthful to show, so fall through rather than bail.
      }
      if (cancelled) return;
      try {
        applyStatus(await api.getKeepAwake());
      } catch (loadError) {
        reportLoadError(loadError);
      }
    }
    void start();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    if (!status?.active || status.options.endsAtMs === null) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [status?.active, status?.options.endsAtMs]);

  const phasePending = status?.phase === 'enabling' || status?.phase === 'disabling';
  const busy = commandBusy || phasePending;

  // Transitions are driven entirely by "tomari:keep-awake-changed": the backend
  // emits it for every change it makes, so the command's own return value is
  // deliberately discarded. Applying it too would let the snapshot it captured
  // *before* spawning the background worker overwrite the newer event that
  // worker has already emitted — leaving the panel stuck showing enabling or
  // disabling for a transition that finished.
  async function run(command: () => Promise<KeepAwakeStatus>) {
    if (commandBusy) return;
    setCommandBusy(true);
    setError(null);
    try {
      await command();
    } catch (e) {
      setError(formatCmdError(e, t));
      try {
        // The command may have failed before changing anything, so no event is
        // coming: re-read instead of leaving the panel on an optimistic draft.
        applyStatus(await api.getKeepAwake());
      } catch {
        // Preserve the last event-backed status when re-sync is unavailable.
      }
    } finally {
      setCommandBusy(false);
    }
  }

  function toggle(next: boolean) {
    if (busy) return;
    // `draftOptions` already holds every edit (each one is saved as it is made
    // and echoed back by the event), so send it as-is. Re-deriving the deadline
    // from the local controls here would resurrect an end time the backend has
    // since dropped as spent.
    void run(() => api.setKeepAwake(next, draftOptions));
  }

  // Option writes are cheap, last-write-wins on the backend, and never prompt,
  // so they bypass the `run` gate: dropping one because a transition happened to
  // be in flight would leave the panel showing an edit the backend never got.
  function saveOptions(next: KeepAwakeOptions) {
    setDraftOptions(next);
    api.configureKeepAwake(next).catch((e: unknown) => setError(formatCmdError(e, t)));
  }

  function changeTimer(nextMode: TimerMode) {
    setTimerMode(nextMode);
    const next = { ...draftOptions, ...timerFields(nextMode, customTime) };
    saveOptions(next);
  }

  // A plain re-read, so unlike a transition its result *is* the new state: it is
  // the only way back when the initial load failed and no event will arrive.
  async function retryStatus() {
    if (busy) return;
    setCommandBusy(true);
    setError(null);
    try {
      applyStatus(await api.getKeepAwake());
    } catch (e) {
      setError(formatCmdError(e, t));
    } finally {
      setCommandBusy(false);
    }
  }

  const active = status?.active ?? false;
  const ready = status !== null;
  const endsAtMs = status?.options.endsAtMs;
  const countdown = active && typeof endsAtMs === 'number' ? formatCountdown(endsAtMs, now) : null;

  return (
    <div className="view session-view">
      <FeaturePageHeader
        title={t('settings.keepAwakeToggle')}
        description={t('settings.sessionPageDescription')}
        checked={active}
        onChange={toggle}
        toggleLabel={busy ? t('settings.working') : t('settings.keepAwakeAction')}
        toggleDisabled={busy || !ready}
        onLabel={t('common.on')}
        offLabel={t('common.off')}
      />

      <div className={`session-state session-state--${status?.phase ?? 'off'}`}>
        <span className="session-state__mark" aria-hidden="true" />
        <div className="session-state__copy">
          <strong>{ready ? t(PHASE_LABEL[status.phase]) : t('common.loading')}</strong>
          <p>{t('settings.currentSessionHint')}</p>
        </div>
        {countdown && (
          <div className="session-state__countdown" aria-label={t('settings.timeRemaining')}>
            <span>{t('settings.timeRemaining')}</span>
            <strong>{countdown}</strong>
          </div>
        )}
      </div>

      {phasePending && (
        <output className="banner">
          <div className="banner__body">
            <strong>{t('settings.authorizationPending')}</strong>
            <p>{t('settings.authorizationPendingHint')}</p>
          </div>
          <button
            type="button"
            className="btn btn--ghost"
            disabled={commandBusy}
            onClick={() => void run(api.cancelKeepAwakeTransition)}
          >
            {t('common.cancel')}
          </button>
        </output>
      )}

      {status?.notice && (
        <div className="banner" role="alert">
          <div className="banner__body">
            <strong>{t(NOTICE_LABEL[status.notice])}</strong>
          </div>
          {status.phase === 'failed' && (
            <button
              type="button"
              className="btn btn--ghost"
              disabled={busy}
              onClick={() => void run(api.retryKeepAwakeTransition)}
            >
              {t('common.retry')}
            </button>
          )}
        </div>
      )}

      {error && (
        <div className="alert" role="alert">
          <span>{error}</span>
          {!ready && (
            <button type="button" className="btn btn--ghost" onClick={() => void retryStatus()}>
              {t('common.retry')}
            </button>
          )}
        </div>
      )}

      <SafetySettings
        t={t}
        busy={busy}
        timerMode={timerMode}
        customTime={customTime}
        options={draftOptions}
        onTimerChange={changeTimer}
        onCustomTimeChange={(value) => {
          setCustomTime(value);
          // Also saved when the field is cleared — otherwise emptying it would
          // leave the backend enforcing the end time the panel no longer shows.
          saveOptions({
            ...draftOptions,
            durationSecs: null,
            endsAtMs: deadlineFor('time', value),
          });
        }}
        onOptionsChange={(overrides) => saveOptions({ ...draftOptions, ...overrides })}
      />
      <SystemState t={t} status={status} />
      <DetectedJobs t={t} status={status} />
    </div>
  );
}

function SafetySettings({
  t,
  busy,
  timerMode,
  customTime,
  options,
  onTimerChange,
  onCustomTimeChange,
  onOptionsChange,
}: {
  t: Translator;
  busy: boolean;
  timerMode: TimerMode;
  customTime: string;
  options: KeepAwakeOptions;
  onTimerChange: (mode: TimerMode) => void;
  onCustomTimeChange: (value: string) => void;
  onOptionsChange: (overrides: Partial<KeepAwakeOptions>) => void;
}) {
  return (
    <SettingsList label={t('settings.safetyGuards')}>
      <SettingsRow
        title={t('settings.autoOff')}
        description={t('settings.autoOffHint')}
        trail={
          <select
            className="input"
            aria-label={t('settings.autoOff')}
            value={timerMode}
            disabled={busy}
            onChange={(event) => onTimerChange(event.target.value as TimerMode)}
          >
            <option value="never">{t('settings.autoOffNever')}</option>
            <option value="30m">{t('settings.autoOff30m')}</option>
            <option value="1h">{t('settings.autoOff1h')}</option>
            <option value="time">{t('settings.autoOffAtTime')}</option>
          </select>
        }
      />
      {timerMode === 'time' && (
        <SettingsRow
          title={t('settings.endTime')}
          trail={
            <input
              className="input"
              type="datetime-local"
              aria-label={t('settings.endTime')}
              value={customTime}
              disabled={busy}
              onChange={(event) => onCustomTimeChange(event.target.value)}
            />
          }
        />
      )}
      <SettingsRow
        title={t('settings.acOnly')}
        description={t('settings.acOnlyHint')}
        trail={
          <Toggle
            checked={options.acOnly}
            label={t('settings.acOnly')}
            disabled={busy}
            onChange={(acOnly) => onOptionsChange({ acOnly })}
          />
        }
      />
      <SettingsRow
        title={t('settings.lowBattery')}
        description={t('settings.lowBatteryHint')}
        trail={
          <select
            className="input"
            aria-label={t('settings.lowBattery')}
            value={options.lowBatteryAction}
            disabled={busy}
            onChange={(event) =>
              onOptionsChange({
                lowBatteryAction: event.target.value as KeepAwakeOptions['lowBatteryAction'],
              })
            }
          >
            <option value="warn">{t('settings.lowBatteryWarn')}</option>
            <option value="turnOff">{t('settings.lowBatteryTurnOff')}</option>
          </select>
        }
      />
    </SettingsList>
  );
}

function SystemState({ t, status }: { t: Translator; status: KeepAwakeStatus | null }) {
  const lid = status?.lidClose ?? 'off';
  const lidChip = LID_CLOSE_CHIP[lid];
  const power = status?.powerSource ?? 'unknown';
  const powerLabel =
    status?.batteryPercent !== null && status?.batteryPercent !== undefined
      ? `${t(POWER_LABEL[power])} · ${status.batteryPercent}%`
      : t(POWER_LABEL[power]);
  return (
    <SettingsList label={t('settings.systemState')}>
      <SettingsRow
        title={t('settings.powerSource')}
        trail={<Chip tone="muted">{powerLabel}</Chip>}
      />
      <SettingsRow
        title={t('settings.kernelState')}
        description={t('settings.kernelStateHint')}
        trail={
          <Chip tone={status?.kernelSleepDisabled ? 'ok' : 'muted'}>
            {status?.kernelSleepDisabled === null || !status
              ? t('settings.systemUnknown')
              : status.kernelSleepDisabled
                ? t('settings.kernelBlocked')
                : t('settings.kernelAllowed')}
          </Chip>
        }
      />
      <SettingsRow
        title={t('settings.lidClose')}
        trail={<Chip tone={lidChip.tone}>{t(lidChip.key)}</Chip>}
      />
      <SettingsRow
        title={t('settings.ownership')}
        description={t('settings.ownershipHint')}
        trail={
          <Chip tone={status?.ownsLidClose ? 'ok' : 'muted'}>
            {status?.ownsLidClose ? t('settings.ownedByTomari') : t('settings.notOwned')}
          </Chip>
        }
      />
    </SettingsList>
  );
}

function DetectedJobs({ t, status }: { t: Translator; status: KeepAwakeStatus | null }) {
  return (
    <SettingsList label={t('settings.detectedJobs')} description={t('settings.detectedJobsHint')}>
      {status?.longRunningProcesses.length ? (
        status.longRunningProcesses.map((process) => (
          <SettingsRow
            key={process.pid}
            title={process.name}
            description={t('settings.processDetail', {
              pid: process.pid,
              elapsed: formatElapsed(process.elapsedSecs),
            })}
          />
        ))
      ) : (
        <SettingsRow description={t('settings.noDetectedJobs')} />
      )}
    </SettingsList>
  );
}
