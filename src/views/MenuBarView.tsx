import { listen } from '@tauri-apps/api/event';
import { useCallback, useEffect, useRef, useState } from 'react';

import {
  FeatureContent,
  FeaturePageHeader,
  SegmentedPageNav,
  SettingsList,
  SettingsRow,
  StatusLabel,
} from '../components/ui';
import * as api from '../lib/api';
import { useT } from '../lib/i18n';
import { useSettings } from '../lib/settings';
import type { MenuBarInventory, MenuBarItem, MenuBarStatus } from '../lib/types';

const AUTO_COLLAPSE_CHOICES = [0, 5, 15, 30] as const;
type MenuBarTab = 'items' | 'behavior';

export function MenuBarView() {
  const t = useT();
  const { settings, update } = useSettings();
  const [tab, setTab] = useState<MenuBarTab>('items');
  const [collapsed, setCollapsed] = useState(true);
  const [busy, setBusy] = useState(false);
  const [inventory, setInventory] = useState<MenuBarInventory | null>(null);
  const [inventoryBusy, setInventoryBusy] = useState(false);
  const [inventoryError, setInventoryError] = useState(false);
  const runtimeEnabled = useRef<boolean | null>(null);
  const inventoryRequest = useRef(0);
  const activeInventoryRequest = useRef<number | null>(null);

  const refreshInventory = useCallback(async () => {
    const request = ++inventoryRequest.current;
    activeInventoryRequest.current = request;
    setInventoryBusy(true);
    setInventoryError(false);
    try {
      const next = await api.listMenuBarItems();
      if (request === inventoryRequest.current) setInventory(next);
    } catch {
      if (request === inventoryRequest.current) setInventoryError(true);
    } finally {
      if (activeInventoryRequest.current === request) activeInventoryRequest.current = null;
      setInventoryBusy(activeInventoryRequest.current !== null);
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    let eventApplied = false;
    const applyStatus = (status: MenuBarStatus) => {
      if (cancelled) return;
      setCollapsed(status.collapsed);
      const wasEnabled = runtimeEnabled.current;
      runtimeEnabled.current = status.enabled;
      if (status.enabled && wasEnabled !== true) {
        void refreshInventory();
      } else if (!status.enabled) {
        // An in-flight scan belongs to the old runtime. Invalidate it so an
        // off/on cycle never exposes inventory from before the new divider.
        inventoryRequest.current += 1;
        activeInventoryRequest.current = null;
        setInventory(null);
        setInventoryBusy(false);
        setInventoryError(false);
      }
    };

    const unlisten = listen<MenuBarStatus>('tomari:menu-bar-changed', (event) => {
      eventApplied = true;
      applyStatus(event.payload);
    });
    const pullStatus = () => {
      if (cancelled) return;
      void api
        .getMenuBar()
        .then((status) => {
          // A runtime event is newer than this startup pull even if the command
          // response happens to arrive last.
          if (!eventApplied) applyStatus(status);
          return undefined;
        })
        .catch(() => {});
    };
    // Establish the event stream before taking the initial snapshot. Otherwise
    // an enable published between the pull and listener registration is lost.
    void unlisten.then(pullStatus, pullStatus);
    return () => {
      cancelled = true;
      runtimeEnabled.current = null;
      inventoryRequest.current += 1;
      activeInventoryRequest.current = null;
      void unlisten.then((fn) => fn());
    };
  }, [refreshInventory]);

  async function show(next: boolean) {
    if (busy) return;
    setBusy(true);
    try {
      const status = await api.setMenuBarCollapsed(!next);
      setCollapsed(status.collapsed);
    } catch {
      // Runtime state remains backend-owned; keep the last confirmed value.
    } finally {
      setBusy(false);
    }
  }

  if (!settings) return <div className="view">{t('common.loading')}</div>;

  const enabled = settings.menuBarTidyEnabled;
  return (
    <div className="view">
      <FeaturePageHeader
        title={t('menubar.title')}
        description={t('menubar.pageDescription')}
        checked={enabled}
        onChange={(next) => update({ menuBarTidyEnabled: next })}
        toggleLabel={t('menubar.enable')}
        onLabel={t('common.on')}
        offLabel={t('common.off')}
      />

      <SegmentedPageNav
        label={t('menubar.tabsLabel')}
        idBase="menubar-tabs"
        value={tab}
        onChange={setTab}
        items={[
          { value: 'items', label: t('menubar.tab.items') },
          { value: 'behavior', label: t('menubar.tab.behavior') },
        ]}
      />

      <FeatureContent enabled={enabled}>
        {tab === 'items' ? (
          <MenuBarItemsPanel
            inventory={inventory}
            busy={inventoryBusy}
            failed={inventoryError}
            onRefresh={() => void refreshInventory()}
          />
        ) : (
          <MenuBarBehaviorPanel
            collapsed={collapsed}
            busy={busy}
            autoCollapseSecs={settings.menuBarAutoCollapseSecs}
            onShow={(next) => void show(next)}
            onAutoCollapse={(seconds) => update({ menuBarAutoCollapseSecs: seconds })}
          />
        )}
      </FeatureContent>
    </div>
  );
}

function MenuBarItemsPanel({
  inventory,
  busy,
  failed,
  onRefresh,
}: {
  inventory: MenuBarInventory | null;
  busy: boolean;
  failed: boolean;
  onRefresh: () => void;
}) {
  const t = useT();
  const usable = inventory?.supported && inventory.permissionGranted && inventory.dividerAvailable;
  const hidden = usable ? inventory.items.filter((item) => item.zone === 'hidden') : [];
  const visible = usable ? inventory.items.filter((item) => item.zone === 'visible') : [];

  return (
    <div
      id="menubar-tabs-panel"
      className="tab-panel"
      role="tabpanel"
      aria-labelledby="menubar-tabs-items-tab"
    >
      <MenuBarDiagram hidden={hidden} visible={visible} />

      <div className="arrangement-action">
        <kbd>⌘</kbd>
        <span>{t('menubar.arrangeInstruction')}</span>
        <button type="button" className="btn" onClick={onRefresh} disabled={busy}>
          {t('menubar.refreshItems')}
        </button>
      </div>

      <InventoryBody inventory={inventory} busy={busy} failed={failed} />
    </div>
  );
}

function MenuBarDiagram({ hidden, visible }: { hidden: MenuBarItem[]; visible: MenuBarItem[] }) {
  const t = useT();
  return (
    <figure className="menu-bar-stage" aria-label={t('menubar.diagramLabel')}>
      <figcaption>{t('menubar.diagramLabel')}</figcaption>
      <div className="menu-bar-strip">
        <div className="menu-bar-strip__zone menu-bar-strip__zone--hidden">
          <span className="menu-bar-strip__zone-label">{t('menubar.zoneHidden')}</span>
          {hidden.slice(0, 4).map((item) => (
            <MenuBarGlyph item={item} key={item.id} />
          ))}
        </div>
        <span className="menu-bar-divider" aria-label="Tomari">
          ≡
        </span>
        <div className="menu-bar-strip__zone menu-bar-strip__zone--visible">
          {visible.slice(0, 5).map((item) => (
            <MenuBarGlyph item={item} key={item.id} />
          ))}
          <span className="menu-bar-strip__zone-label">{t('menubar.zoneVisible')}</span>
        </div>
      </div>
    </figure>
  );
}

function MenuBarGlyph({ item }: { item: MenuBarItem }) {
  return (
    <span className="menu-bar-glyph" title={item.name}>
      {Array.from(item.name.trim())[0]?.toLocaleUpperCase() ?? '•'}
    </span>
  );
}

function InventoryBody({
  inventory,
  busy,
  failed,
}: {
  inventory: MenuBarInventory | null;
  busy: boolean;
  failed: boolean;
}) {
  const t = useT();
  if (busy && !inventory) return <p className="inventory-state">{t('menubar.inventoryLoading')}</p>;
  if (failed)
    return <p className="inventory-state inventory-state--error">{t('menubar.inventoryError')}</p>;
  if (!inventory) return null;
  if (!inventory.supported)
    return <p className="inventory-state">{t('menubar.inventoryUnsupported')}</p>;
  if (!inventory.permissionGranted)
    return <p className="inventory-state">{t('menubar.inventoryPermission')}</p>;
  if (!inventory.dividerAvailable)
    return <p className="inventory-state">{t('menubar.inventoryDividerMissing')}</p>;

  const hidden = inventory.items.filter((item) => item.zone === 'hidden');
  const visible = inventory.items.filter((item) => item.zone === 'visible');
  return (
    <div className="inventory-grid" aria-live="polite">
      <InventorySection title={t('menubar.hiddenItems')} items={hidden} tone="active" />
      <InventorySection title={t('menubar.visibleItems')} items={visible} tone="muted" />
    </div>
  );
}

function InventorySection({
  title,
  items,
  tone,
}: {
  title: string;
  items: MenuBarItem[];
  tone: 'active' | 'muted';
}) {
  const t = useT();
  return (
    <section className="inventory-column">
      <header>
        <h2>{title}</h2>
        <span>{t('menubar.itemCount', { count: String(items.length) })}</span>
      </header>
      <div className="inventory-column__body">
        {items.length === 0 ? (
          <p className="inventory-empty">{t('menubar.inventoryEmpty')}</p>
        ) : (
          items.map((item) => (
            <div className="inventory-item" key={item.id}>
              <span className="inventory-item__glyph" aria-hidden="true">
                {Array.from(item.name.trim())[0]?.toLocaleUpperCase() ?? '•'}
              </span>
              <span className="inventory-item__copy">
                <strong>{item.name}</strong>
                {item.ownerName && (
                  <small>{t('menubar.itemOwner', { owner: item.ownerName })}</small>
                )}
              </span>
              <StatusLabel tone={tone}>
                {t(tone === 'active' ? 'menubar.zoneHidden' : 'menubar.zoneVisible')}
              </StatusLabel>
            </div>
          ))
        )}
      </div>
    </section>
  );
}

function MenuBarBehaviorPanel({
  collapsed,
  busy,
  autoCollapseSecs,
  onShow,
  onAutoCollapse,
}: {
  collapsed: boolean;
  busy: boolean;
  autoCollapseSecs: number;
  onShow: (next: boolean) => void;
  onAutoCollapse: (seconds: number) => void;
}) {
  const t = useT();
  return (
    <div
      id="menubar-tabs-panel"
      className="tab-panel"
      role="tabpanel"
      aria-labelledby="menubar-tabs-behavior-tab"
    >
      <div className={`visibility-stage ${collapsed ? 'visibility-stage--collapsed' : ''}`}>
        <div className="visibility-stage__bar">
          <span className="visibility-stage__hidden">•••</span>
          <span className="menu-bar-divider">≡</span>
          <span>Wi-Fi&nbsp;&nbsp;◒&nbsp;&nbsp;12:34</span>
        </div>
        <strong>{t(collapsed ? 'menubar.iconsHidden' : 'menubar.iconsVisible')}</strong>
      </div>

      <SettingsList>
        <SettingsRow
          title={t(collapsed ? 'menubar.iconsHidden' : 'menubar.iconsVisible')}
          trail={
            <button type="button" className="btn" disabled={busy} onClick={() => onShow(collapsed)}>
              {t(collapsed ? 'menubar.showAction' : 'menubar.hideAction')}
            </button>
          }
        />
        <SettingsRow
          title={t('menubar.autoCollapse')}
          trail={
            <select
              className="input"
              value={autoCollapseSecs}
              onChange={(event) => onAutoCollapse(Number(event.target.value))}
              aria-label={t('menubar.autoCollapse')}
            >
              {AUTO_COLLAPSE_CHOICES.map((seconds) => (
                <option key={seconds} value={seconds}>
                  {seconds === 0
                    ? t('menubar.autoCollapseNever')
                    : t('menubar.autoCollapseSecs', { secs: String(seconds) })}
                </option>
              ))}
            </select>
          }
        />
      </SettingsList>
    </div>
  );
}
