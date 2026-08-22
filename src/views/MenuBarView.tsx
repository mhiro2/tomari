import { listen } from '@tauri-apps/api/event';
import { useEffect, useState } from 'react';

import { Chip, Group, MasterSwitchHeader, Toggle } from '../components/ui';
import * as api from '../lib/api';
import { useT } from '../lib/i18n';
import { useSettings } from '../lib/settings';
import type { MenuBarInventory, MenuBarItem, MenuBarStatus } from '../lib/types';

/** The auto-collapse delays offered; 0 means the timer is off. */
const AUTO_COLLAPSE_CHOICES = [0, 5, 15, 30] as const;

export function MenuBarView() {
  const t = useT();
  const { settings, update } = useSettings();
  const [collapsed, setCollapsed] = useState(true);
  const [busy, setBusy] = useState(false);
  const [inventory, setInventory] = useState<MenuBarInventory | null>(null);
  const [inventoryBusy, setInventoryBusy] = useState(false);
  const [inventoryError, setInventoryError] = useState(false);

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

  useEffect(() => {
    if (!settings?.menuBarTidyEnabled) {
      setInventory(null);
      return;
    }
    let cancelled = false;
    setInventoryBusy(true);
    setInventoryError(false);
    async function loadInventory() {
      try {
        const next = await api.listMenuBarItems();
        if (!cancelled) setInventory(next);
      } catch {
        if (!cancelled) setInventoryError(true);
      } finally {
        if (!cancelled) setInventoryBusy(false);
      }
    }
    void loadInventory();
    return () => {
      cancelled = true;
    };
  }, [settings?.menuBarTidyEnabled]);

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

  async function refreshInventory() {
    if (inventoryBusy) return;
    setInventoryBusy(true);
    setInventoryError(false);
    try {
      const next = await api.listMenuBarItems();
      setInventory(next);
    } catch {
      setInventoryError(true);
    } finally {
      setInventoryBusy(false);
    }
  }

  async function grantAccessibility() {
    try {
      await api.requestAccessibility();
      await refreshInventory();
    } catch {
      setInventoryError(true);
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

        {/* The physical arrangement stays authoritative. Accessibility lets the
            settings panel mirror it without pretending another app's status
            items can be moved through a supported AppKit API. */}
        <Group label={t('menubar.arrangeSection')} note={t('menubar.limitNote')}>
          <div className="item mb-inventory__intro">
            <div className="item__body">
              <span className="item__desc">{t('menubar.inventoryIntro')}</span>
            </div>
            <div className="item__trail">
              <button
                type="button"
                className="btn btn--ghost"
                onClick={() => void refreshInventory()}
                disabled={inventoryBusy}
              >
                {t('menubar.refreshItems')}
              </button>
            </div>
          </div>
          <InventoryBody
            inventory={inventory}
            busy={inventoryBusy}
            failed={inventoryError}
            onGrant={() => void grantAccessibility()}
          />
        </Group>
      </div>
    </div>
  );
}

function InventoryBody({
  inventory,
  busy,
  failed,
  onGrant,
}: {
  inventory: MenuBarInventory | null;
  busy: boolean;
  failed: boolean;
  onGrant: () => void;
}) {
  const t = useT();
  if (busy && !inventory) {
    return <p className="mb-inventory__state">{t('menubar.inventoryLoading')}</p>;
  }
  if (failed) {
    return (
      <p className="mb-inventory__state mb-inventory__state--error">
        {t('menubar.inventoryError')}
      </p>
    );
  }
  if (!inventory) return null;
  if (!inventory.supported) {
    return <p className="mb-inventory__state">{t('menubar.inventoryUnsupported')}</p>;
  }
  if (!inventory.permissionGranted) {
    return (
      <div className="mb-inventory__permission">
        <span>{t('menubar.inventoryPermission')}</span>
        <button type="button" className="btn btn--amber" onClick={onGrant}>
          {t('menubar.grantAccessibility')}
        </button>
      </div>
    );
  }
  if (!inventory.dividerAvailable) {
    return <p className="mb-inventory__state">{t('menubar.inventoryDividerMissing')}</p>;
  }

  const hidden = inventory.items.filter((item) => item.zone === 'hidden');
  const visible = inventory.items.filter((item) => item.zone === 'visible');
  return (
    <div className="mb-inventory" aria-live="polite">
      <div className="mb-map" aria-hidden="true">
        <span className="mb-map__zone">
          {t('menubar.zoneHidden')} · {hidden.length}
        </span>
        <span className="mb-map__divider">≡</span>
        <span className="mb-map__zone">
          {t('menubar.zoneVisible')} · {visible.length}
        </span>
      </div>
      <InventorySection title={t('menubar.hiddenItems')} items={hidden} hidden />
      <InventorySection title={t('menubar.visibleItems')} items={visible} />
    </div>
  );
}

function InventorySection({
  title,
  items,
  hidden = false,
}: {
  title: string;
  items: MenuBarItem[];
  hidden?: boolean;
}) {
  const t = useT();
  return (
    <section className="mb-inventory__section">
      <header className="mb-inventory__header">
        <span>{title}</span>
        <span>{t('menubar.itemCount', { count: String(items.length) })}</span>
      </header>
      {items.length === 0 ? (
        <p className="mb-inventory__empty">{t('menubar.inventoryEmpty')}</p>
      ) : (
        items.map((item) => (
          <div className="mb-inventory__item" key={item.id}>
            <span className="mb-inventory__glyph" aria-hidden="true">
              {Array.from(item.name.trim())[0]?.toLocaleUpperCase() ?? '•'}
            </span>
            <span className="mb-inventory__identity">
              <span className="mb-inventory__name">{item.name}</span>
              {item.ownerName && (
                <span className="mb-inventory__owner">
                  {t('menubar.itemOwner', { owner: item.ownerName })}
                </span>
              )}
            </span>
            <Chip tone={hidden ? 'on' : 'muted'}>
              {t(hidden ? 'menubar.zoneHidden' : 'menubar.zoneVisible')}
            </Chip>
          </div>
        ))
      )}
    </section>
  );
}
