import { listen } from '@tauri-apps/api/event';
import { useEffect, useEffectEvent, useState } from 'react';

import { Chip, FeaturePageHeader, SettingsList, SettingsRow } from '../components/ui';
import * as api from '../lib/api';
import { formatCmdError } from '../lib/errors';
import { useT, type MessageKey } from '../lib/i18n';
import type { KeepAwakeStatus, LidCloseState } from '../lib/types';

const LID_CLOSE_CHIP: Record<
  LidCloseState,
  { tone: 'ok' | 'warn' | 'err' | 'muted'; key: MessageKey }
> = {
  engaged: { tone: 'ok', key: 'settings.lidActive' },
  pending: { tone: 'warn', key: 'settings.lidPending' },
  unavailable: { tone: 'err', key: 'settings.lidUnavailable' },
  off: { tone: 'muted', key: 'settings.lidOff' },
};

// Keep-awake is runtime-only state (it never persists), so it lives outside
// AppSettings: this view fetches it on open and follows the
// "tomari:keep-awake-changed" event so the tray and the panel stay in sync.
export function SessionView() {
  const t = useT();
  const [status, setStatus] = useState<KeepAwakeStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // The mount effect must not re-run on language change, so it reads the
  // translator through an effect event.
  const reportLoadError = useEffectEvent((e: unknown) => setError(formatCmdError(e, t)));

  useEffect(() => {
    api
      .getKeepAwake()
      .then(setStatus)
      .catch((e: unknown) => reportLoadError(e));
    const unlisten = listen<KeepAwakeStatus>('tomari:keep-awake-changed', (e) =>
      setStatus(e.payload),
    );
    return () => void unlisten.then((fn) => fn());
  }, []);

  async function toggle(next: boolean) {
    // Turning keep-awake on prompts for the admin password, so a second call
    // while one is in flight must be ignored rather than queued.
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      setStatus(await api.setKeepAwake(next));
    } catch (e) {
      setError(formatCmdError(e, t));
      try {
        // Re-sync from the backend if the toggle could not be applied.
        setStatus(await api.getKeepAwake());
      } catch {
        // Keep the last known status if the re-sync itself fails.
      }
    } finally {
      setBusy(false);
    }
  }

  async function retryStatus() {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      setStatus(await api.getKeepAwake());
    } catch (e) {
      setError(formatCmdError(e, t));
    } finally {
      setBusy(false);
    }
  }

  const active = status?.active ?? false;
  const ready = status !== null;
  const lid = status?.lidClose ?? 'off';
  const chip = LID_CLOSE_CHIP[lid];

  return (
    <div className="view">
      <FeaturePageHeader
        title={t('settings.keepAwakeToggle')}
        description={t('settings.sessionPageDescription')}
        checked={active}
        onChange={(next) => void toggle(next)}
        toggleLabel={busy ? t('settings.working') : t('settings.keepAwakeAction')}
        toggleDisabled={busy || !ready}
        onLabel={t('common.on')}
        offLabel={t('common.off')}
      />

      <div className={`session-state ${active ? 'session-state--active' : ''}`}>
        <span className="session-state__mark" aria-hidden="true" />
        <div>
          <strong>
            {ready
              ? active
                ? t('settings.keepAwakeActive')
                : t('settings.keepAwakeInactive')
              : t('common.loading')}
          </strong>
          <p>{t('settings.currentSessionHint')}</p>
        </div>
      </div>

      <SettingsList>
        <SettingsRow
          title={t('settings.keepAwakeAction')}
          description={t('settings.keepAwakeHint')}
        />
        {error && (
          <SettingsRow
            description={
              <span className="hint--err" role="alert">
                {error}
              </span>
            }
            trail={
              !ready && (
                <button
                  type="button"
                  className="btn btn--ghost"
                  onClick={() => void retryStatus()}
                  disabled={busy}
                >
                  {t('common.retry')}
                </button>
              )
            }
          />
        )}
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
