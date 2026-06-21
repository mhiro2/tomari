import { listen } from '@tauri-apps/api/event';
import { useEffect, useState } from 'react';

import { Chip, Group, SwitchRow } from '../components/ui';
import * as api from '../lib/api';
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

  useEffect(() => {
    void api.getKeepAwake().then(setStatus);
    const unlisten = listen<KeepAwakeStatus>('tomari:keep-awake-changed', (e) =>
      setStatus(e.payload),
    );
    return () => void unlisten.then((fn) => fn());
  }, []);

  async function toggle(next: boolean) {
    setBusy(true);
    try {
      setStatus(await api.setKeepAwake(next));
    } catch {
      // Re-sync from the backend if the toggle could not be applied.
      setStatus(await api.getKeepAwake());
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
        <SwitchRow
          title={t('settings.keepAwakeToggle')}
          desc={t('settings.keepAwakeHint')}
          checked={active}
          onChange={(v) => void toggle(v)}
          toggleLabel={busy ? t('settings.working') : undefined}
        />
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
