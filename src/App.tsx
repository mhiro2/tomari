import { listen } from '@tauri-apps/api/event';
import { useCallback, useEffect, useState } from 'react';

import { formatCmdError } from './lib/errors';
import { I18nProvider, resolveLang, useT } from './lib/i18n';
import { SettingsProvider, useSettings } from './lib/settings';
import type { Theme } from './lib/types';
import { KeyboardView } from './views/KeyboardView';
import { SettingsView } from './views/SettingsView';
import { WindowView } from './views/WindowView';

type Tab = 'keyboard' | 'window' | 'settings';

const TABS: Tab[] = ['keyboard', 'window', 'settings'];

const TAB_ICONS: Record<Tab, string> = {
  keyboard: '⌨',
  window: '▦',
  settings: '⚙',
};

function applyTheme(theme: Theme) {
  const root = document.documentElement;
  if (theme === 'system') {
    const dark = globalThis.matchMedia('(prefers-color-scheme: dark)').matches;
    root.dataset.theme = dark ? 'dark' : 'light';
  } else {
    root.dataset.theme = theme;
  }
}

export function App() {
  return (
    <SettingsProvider>
      <Localized />
    </SettingsProvider>
  );
}

// Drives theme + UI language off the shared settings record. Theme and language
// both come from the same source now, so there is no separate fetch to keep in
// sync with the rest of the panel.
function Localized() {
  const { settings } = useSettings();
  const theme = settings?.theme ?? 'system';

  // Apply the theme, and — while it follows the system — re-apply when the OS
  // appearance flips. The panel webview stays alive across the change, so
  // without this a System theme would only update on the next relaunch.
  useEffect(() => {
    applyTheme(theme);
    if (theme !== 'system') return;
    const mql = globalThis.matchMedia('(prefers-color-scheme: dark)');
    const onChange = () => applyTheme('system');
    mql.addEventListener('change', onChange);
    return () => mql.removeEventListener('change', onChange);
  }, [theme]);

  return (
    <I18nProvider lang={resolveLang(settings?.language ?? 'system')}>
      <AppShell />
    </I18nProvider>
  );
}

function AppShell() {
  const t = useT();
  const { settings, saveError } = useSettings();
  const [tab, setTab] = useState<Tab>('keyboard');
  const [autoCheckUpdate, setAutoCheckUpdate] = useState(false);

  // The tray's "Check for Updates" entry opens this panel and emits an event;
  // jump to Settings and have it run the check.
  useEffect(() => {
    const unlisten = listen('tomari:check-update', () => {
      setTab('settings');
      setAutoCheckUpdate(true);
    });
    return () => void unlisten.then((fn) => fn());
  }, []);

  const onAutoCheckHandled = useCallback(() => setAutoCheckUpdate(false), []);

  // A muted dot on a tab whose feature is switched off, so the master switch
  // (which lives inside the tab) is discoverable from the nav.
  const featureOff: Record<Tab, boolean> = {
    keyboard: settings ? !settings.keyboardEnabled : false,
    window: settings ? !settings.windowManagementEnabled : false,
    settings: false,
  };

  return (
    <div className="app">
      {/* The frameless window has no title bar, so the header doubles as the
          drag handle. */}
      <header className="app__header" data-tauri-drag-region>
        <div className="brand" data-tauri-drag-region>
          <span className="brand__mark" aria-hidden="true">
            ◆
          </span>
          <span className="brand__name">Tomari</span>
        </div>
      </header>

      <nav className="tabs" aria-label={t('app.sections')}>
        {TABS.map((id) => (
          <button
            key={id}
            type="button"
            className={`tab ${tab === id ? 'tab--active' : ''}`}
            aria-current={tab === id}
            // When off, fold the state into the accessible name; the dot itself
            // is decorative.
            aria-label={
              featureOff[id] ? `${t(`app.tabs.${id}`)} (${t('app.featureOff')})` : undefined
            }
            onClick={() => setTab(id)}
          >
            <span className="tab__icon" aria-hidden="true">
              {TAB_ICONS[id]}
            </span>
            {t(`app.tabs.${id}`)}
            {featureOff[id] && <span className="tab__dot" aria-hidden="true" />}
          </button>
        ))}
      </nav>

      {saveError !== null && (
        <p className="alert" role="alert">
          {t('settings.saveFailed', { error: formatCmdError(saveError, t) })}
        </p>
      )}

      <main className="app__main">
        {tab === 'keyboard' && <KeyboardView />}
        {tab === 'window' && <WindowView />}
        {tab === 'settings' && (
          <SettingsView autoCheckUpdate={autoCheckUpdate} onAutoCheckHandled={onAutoCheckHandled} />
        )}
      </main>
    </div>
  );
}
