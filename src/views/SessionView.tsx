import { listen } from '@tauri-apps/api/event';
import { useEffect, useState } from 'react';

import { Chip, Group, Toggle } from '../components/ui';
import * as api from '../lib/api';
import { formatCmdError } from '../lib/errors';
import { useT, type Translator } from '../lib/i18n';
import type { KeepAwakeStatus, LidCloseState } from '../lib/types';

const LID_CLOSE_CHIP: Record<
  LidCloseState,
  { tone: 'ok' | 'warn' | 'err' | 'muted'; key: string }
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

  useEffect(() => {
    void api.getKeepAwake().then(setStatus);
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

  const active = status?.active ?? false;
  const lid = status?.lidClose ?? 'off';
  const chip = LID_CLOSE_CHIP[lid];

  return (
    <div className="view">
      <Group>
        {/* SwitchRow has no `disabled` prop, so this row is inlined here to pass
            `disabled` straight to Toggle and keep the busy guard from being
            bypassed by a stray click while the admin prompt is in flight. */}
        <div className="item">
          <div className="item__body">
            <span className="item__title">{t('settings.keepAwakeToggle')}</span>
            <span className="item__desc">{t('settings.keepAwakeHint')}</span>
          </div>
          <div className="item__trail">
            <Toggle
              checked={active}
              onChange={(v) => void toggle(v)}
              disabled={busy}
              label={busy ? t('settings.working') : t('settings.keepAwakeToggle')}
            />
          </div>
        </div>
        {error && (
          <div className="item">
            <span className="hint--err" role="alert">
              {error}
            </span>
          </div>
        )}
        {active && (
          <div className="item">
            <div className="item__body">
              <span className="item__title">{t('settings.lidClose')}</span>
            </div>
            <div className="item__trail">
              <Chip tone={chip.tone}>{t(chip.key as Parameters<Translator>[0])}</Chip>
            </div>
          </div>
        )}
      </Group>
    </div>
  );
}
