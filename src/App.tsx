import { listen } from '@tauri-apps/api/event';
import { useCallback, useEffect, useState } from 'react';

import { Banner } from './components/ui';
import { formatCmdError } from './lib/errors';
import { I18nProvider, resolveLang, useT } from './lib/i18n';
import { SettingsProvider, useSettings } from './lib/settings';
import { GeneralView } from './views/GeneralView';
import { KeyboardView } from './views/KeyboardView';
import { SessionView } from './views/SessionView';
import { WindowView } from './views/WindowView';

type Tab = 'keyboard' | 'window' | 'session' | 'general';

const TABS: Tab[] = ['keyboard', 'window', 'session', 'general'];

const TAB_ICONS: Record<Tab, string> = {
  keyboard: '⌨',
  window: '▦',
  session: '◉',
  general: '⚙',
};

export function App() {
  return (
    <SettingsProvider>
      <Localized />
    </SettingsProvider>
  );
}

// Drives the UI language off the shared settings record (the app is dark-only,
// so there is no theme to apply).
function Localized() {
  const { settings } = useSettings();

  return (
    <I18nProvider lang={resolveLang(settings?.language ?? 'system')}>
      <AppShell />
    </I18nProvider>
  );
}

function AppShell() {
  const t = useT();
  const { settings, loadError, retryLoad, saveError } = useSettings();
  const [tab, setTab] = useState<Tab>('keyboard');
  const [autoCheckUpdate, setAutoCheckUpdate] = useState(false);

  // The tray "Check for Updates" entry shows this window and emits the event;
  // jump to the General tab and run the check so the result shows up there.
  useEffect(() => {
    const unlisten = listen('tomari:check-update', () => {
      setTab('general');
      setAutoCheckUpdate(true);
    });
    return () => void unlisten.then((fn) => fn());
  }, []);

  const onAutoCheckHandled = useCallback(() => setAutoCheckUpdate(false), []);

  // A muted dot on a tab whose feature is switched off, so the master switch
  // (which lives inside the tab) is discoverable from the nav. The Session and
  // General tabs have no master switch, so they never carry one.
  const featureOff: Record<Tab, boolean> = {
    keyboard: settings ? !settings.keyboardEnabled : false,
    window: settings ? !settings.windowManagementEnabled : false,
    session: false,
    general: false,
  };

  return (
    <div className="app">
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
        {settings === null && loadError !== null ? (
          // The initial settings load failed, so every view would sit on its
          // loading state forever — show the error with a retry instead.
          <Banner tone="warn">
            <div className="banner__body" role="alert">
              <p>{t('common.loadFailed', { error: formatCmdError(loadError, t) })}</p>
            </div>
            <button type="button" className="btn btn--primary" onClick={retryLoad}>
              {t('common.retry')}
            </button>
          </Banner>
        ) : (
          <>
            {tab === 'keyboard' && <KeyboardView />}
            {tab === 'window' && <WindowView />}
            {tab === 'session' && <SessionView />}
            {tab === 'general' && (
              <GeneralView
                autoCheckUpdate={autoCheckUpdate}
                onAutoCheckHandled={onAutoCheckHandled}
              />
            )}
          </>
        )}
      </main>
    </div>
  );
}
