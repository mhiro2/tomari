import { listen } from '@tauri-apps/api/event';
import { useEffect, useState } from 'react';

import { Group, MasterSwitchHeader, Toggle } from '../components/ui';
import * as api from '../lib/api';
import { useT } from '../lib/i18n';
import { useSettings } from '../lib/settings';
import type { MenuBarStatus } from '../lib/types';

/** The auto-collapse delays offered; 0 means the timer is off. */
const AUTO_COLLAPSE_CHOICES = [0, 5, 15, 30] as const;

export function MenuBarView() {
  const t = useT();
  const { settings, update } = useSettings();
  const [collapsed, setCollapsed] = useState(true);
  const [busy, setBusy] = useState(false);

  // The state also changes from the menu bar item itself, the tray and a
  // hotkey, so pull once on mount and follow the event after that.
  useEffect(() => {
    void api
      .getMenuBar()
      .then((s) => setCollapsed(s.collapsed))
      .catch(() => {});
    const unlisten = listen<MenuBarStatus>('tomari:menu-bar-changed', (e) =>
      setCollapsed(e.payload.collapsed),
    );
    return () => void unlisten.then((fn) => fn());
  }, []);

  async function show(next: boolean) {
    if (busy) return;
    setBusy(true);
    try {
      const status = await api.setMenuBarCollapsed(!next);
      setCollapsed(status.collapsed);
    } catch {
      // The backend owns the state and broadcasts every change; a failed call
      // leaves the last known value in place rather than a guess.
    } finally {
      setBusy(false);
    }
  }

  if (!settings) return <div className="view">{t('common.loading')}</div>;

  const on = settings.menuBarTidyEnabled;

  return (
    <div className="view">
      <MasterSwitchHeader
        title={t('menubar.title')}
        checked={on}
        onChange={(v) => update({ menuBarTidyEnabled: v })}
        offNote={t('menubar.offNote')}
        enableLabel={t('common.turnOn')}
        toggleLabel={t('menubar.enable')}
      />

      <div className={`view ${on ? '' : 'gated'}`} aria-disabled={!on} inert={!on}>
        {/* How it is operated. No group label: it sits directly under the
            master switch, which already says what this section is. */}
        <Group>
          {/* SwitchRow has no `disabled` prop, so this row is inlined to pass
              `disabled` through and keep the busy guard from being bypassed. */}
          <div className="item">
            <div className="item__body">
              <span className="item__title">{t('menubar.showToggle')}</span>
              <span className="item__desc">{t('menubar.showDesc')}</span>
            </div>
            <div className="item__trail">
              <Toggle
                checked={!collapsed}
                onChange={(v) => void show(v)}
                disabled={busy}
                label={t('menubar.showToggle')}
              />
            </div>
          </div>
          <div className="item">
            <div className="item__body">
              <span className="item__title">{t('menubar.autoCollapse')}</span>
            </div>
            <div className="item__trail">
              <select
                className="input"
                value={settings.menuBarAutoCollapseSecs}
                onChange={(e) => update({ menuBarAutoCollapseSecs: Number(e.target.value) })}
                aria-label={t('menubar.autoCollapse')}
              >
                {AUTO_COLLAPSE_CHOICES.map((secs) => (
                  <option key={secs} value={secs}>
                    {secs === 0
                      ? t('menubar.autoCollapseNever')
                      : t('menubar.autoCollapseSecs', { secs: String(secs) })}
                  </option>
                ))}
              </select>
            </div>
          </div>
        </Group>

        {/* The part the user has to do by hand, and the only part that needs
            explaining at length. */}
        <Group label={t('menubar.arrangeSection')} note={t('menubar.limitNote')}>
          <div className="item">
            <div className="item__body">
              <span className="item__desc">{t('menubar.arrangeBody')}</span>
              <MenuBarDiagram />
            </div>
          </div>
        </Group>
      </div>
    </div>
  );
}

/**
 * A picture of the arrangement the user has to make by hand. Decorative — the
 * prose above says the same thing — so it is hidden from assistive tech rather
 * than read out as three stray words.
 */
function MenuBarDiagram() {
  const t = useT();
  return (
    <div className="mb-map" aria-hidden="true">
      <span className="mb-map__zone">{t('menubar.zoneHidden')}</span>
      <span className="mb-map__divider">≡</span>
      <span className="mb-map__zone">{t('menubar.zoneVisible')}</span>
    </div>
  );
}
