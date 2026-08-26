import { listen } from '@tauri-apps/api/event';
import { useEffect, useEffectEvent, useRef, useState } from 'react';

import { Chip, FeaturePageHeader, SettingsList, SettingsRow } from '../components/ui';
import * as api from '../lib/api';
import { formatCmdError } from '../lib/errors';
import { useT, type MessageKey } from '../lib/i18n';
import type { KeepAwakeNotice, KeepAwakePhase, KeepAwakeStatus, LidCloseState } from '../lib/types';

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
  authorizationDeclined: 'settings.noticeAuthorizationDeclined',
};

// Keep-awake is runtime-only state (it never persists), so it lives outside
// AppSettings. The backend owns every transition; this view renders that state
// machine and follows its event while the panel is open.
export function SessionView() {
  const t = useT();
  const [status, setStatus] = useState<KeepAwakeStatus | null>(null);
  const [commandBusy, setCommandBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Highest revision applied so far. Several backend threads emit, each
  // snapshotting before it emits, so an older snapshot can still arrive last.
  const appliedRevision = useRef(-1);

  // The mount effect must not re-run on language change, so it reads the
  // translator through an effect event.
  const reportLoadError = useEffectEvent((e: unknown) => setError(formatCmdError(e, t)));

  // The single writer of the rendered state.
  const applyStatus = useEffectEvent((next: KeepAwakeStatus) => {
    // Rendering an older snapshot would strand the panel on a transition that
    // has already finished — with every toggle disabled and no further event
    // coming to release it.
    if (next.revision < appliedRevision.current) return;
    appliedRevision.current = next.revision;
    setStatus(next);
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

  // An administrator prompt outlives the command that opened it, so the pending
  // phase — not the in-flight call — is what locks the switch.
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
        // coming: re-read rather than leave the panel on a guessed state.
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
    void run(() => api.setKeepAwake(next));
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
  const lid = status?.lidClose ?? 'off';
  const chip = LID_CLOSE_CHIP[lid];

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
        <div>
          <strong>{ready ? t(PHASE_LABEL[status.phase]) : t('common.loading')}</strong>
          <p>{t('settings.currentSessionHint')}</p>
        </div>
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

      <SettingsList>
        <SettingsRow
          title={t('settings.keepAwakeAction')}
          description={t('settings.keepAwakeHint')}
        />
        {active && (
          <SettingsRow
            title={t('settings.lidClose')}
            trail={<Chip tone={chip.tone}>{t(chip.key)}</Chip>}
          />
        )}
      </SettingsList>
    </div>
  );
}
