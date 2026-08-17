import { listen } from '@tauri-apps/api/event';
import { useCallback, useEffect, useState } from 'react';

import { SectionIcon, type SectionName } from './components/icons';
import { Banner } from './components/ui';
import * as api from './lib/api';
import { formatCmdError } from './lib/errors';
import { I18nProvider, resolveLang, useT } from './lib/i18n';
import { SettingsProvider, useSettings } from './lib/settings';
import type { PermissionsChanged } from './lib/types';
import { GeneralView } from './views/GeneralView';
import { KeyboardView } from './views/KeyboardView';
import { MenuBarView } from './views/MenuBarView';
import { SessionView } from './views/SessionView';
import { SetupView, type SetupPermissions } from './views/SetupView';
import { WindowView } from './views/WindowView';

type Section = SectionName;

const SECTIONS: Section[] = ['keyboard', 'window', 'menubar', 'session', 'general'];

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
  const lang = resolveLang(settings?.language ?? 'system');

  // Keep the document's declared language in sync so assistive tech and the
  // browser's own heuristics (e.g. find-in-page, spellcheck) match the UI.
  useEffect(() => {
    document.documentElement.lang = lang;
  }, [lang]);

  return (
    <I18nProvider lang={lang}>
      <AppShell />
    </I18nProvider>
  );
}

// Where the setup checklist stands: 'unknown' until the startup pull settles,
// 'open' while it replaces the tabs, 'dismissed' once "Set up later" swapped it
// for the tabs plus a reminder banner, 'done' when nothing is missing.
type SetupState = 'unknown' | 'open' | 'dismissed' | 'done';

function AppShell() {
  const t = useT();
  const { settings, loadError, retryLoad, saveError } = useSettings();
  const [section, setSection] = useState<Section>('keyboard');
  const [autoCheckUpdate, setAutoCheckUpdate] = useState(false);
  const [setup, setSetup] = useState<SetupState>('unknown');
  const [updateRegrant, setUpdateRegrant] = useState(false);
  const [permissions, setPermissions] = useState<SetupPermissions>({
    accessibility: true,
    inputMonitoring: true,
  });

  // The tray "Check for Updates" entry shows this window and emits the event;
  // jump to the General section and run the check so the result shows up there.
  useEffect(() => {
    const unlisten = listen('tomari:check-update', () => {
      setSection('general');
      setAutoCheckUpdate(true);
    });
    return () => void unlisten.then((fn) => fn());
  }, []);

  // Pull the setup status once the WebView is up (an event pushed from the
  // backend at launch could fire before this listener exists, so it is a pull),
  // then keep the permission pair current from the backend's poll. A failed
  // pull falls back to 'done' — the sections, exactly the pre-setup-view
  // behavior.
  useEffect(() => {
    void (async () => {
      try {
        const s = await api.setupStatus();
        setPermissions({ accessibility: s.accessibility, inputMonitoring: s.inputMonitoring });
        setUpdateRegrant(s.updateRegrant);
        const missing = !s.accessibility || !s.inputMonitoring;
        setSetup(missing ? (s.firstRun || s.updateRegrant ? 'open' : 'dismissed') : 'done');
      } catch {
        setSetup('done');
      }
    })();
    const unlisten = listen<PermissionsChanged>('tomari:permissions-changed', (e) =>
      setPermissions({
        accessibility: e.payload.accessibility,
        inputMonitoring: e.payload.inputMonitoring,
      }),
    );
    return () => void unlisten.then((fn) => fn());
  }, []);

  // Once everything is granted, retire the reminder banner on its own; if a
  // permission is later revoked (in System Settings, outside the app), bring
  // it back so the loss is visible even in sections that carry no permission
  // banner of their own. The open checklist is deliberately not auto-closed —
  // it stays to show the ✓s and let the user leave via its Done button.
  useEffect(() => {
    const allGranted = permissions.accessibility && permissions.inputMonitoring;
    if (setup === 'dismissed' && allGranted) setSetup('done');
    if (setup === 'done' && !allGranted) setSetup('dismissed');
  }, [setup, permissions]);

  // The update explanation has served its purpose once setup completes; a
  // checklist reopened after a *later* manual revocation must not still
  // blame the update.
  useEffect(() => {
    if (setup === 'done') setUpdateRegrant(false);
  }, [setup]);

  const onAutoCheckHandled = useCallback(() => setAutoCheckUpdate(false), []);
  const openSetup = useCallback(() => setSetup('open'), []);

  const onGranted = useCallback(
    (patch: Partial<SetupPermissions>) => setPermissions((p) => ({ ...p, ...patch })),
    [],
  );

  // Hold the shell until the setup pull settles: rendering the sections first
  // and swapping them for the checklist a beat later would flicker and yank the
  // DOM out from under anyone who already started reading or tabbing. Setup
  // takes the whole window — the sidebar would offer paths that do not work
  // until the permissions are granted.
  if (setup === 'unknown') {
    return (
      <div className="app">
        <div className="app__content">
          <main className="app__main">
            <div className="view">{t('common.loading')}</div>
          </main>
        </div>
      </div>
    );
  }

  if (setup === 'open') {
    return (
      <div className="app">
        <div className="app__content">
          <main className="app__main">
            <SetupView
              permissions={permissions}
              updateRegrant={updateRegrant}
              onGranted={onGranted}
              onDismiss={() => setSetup('dismissed')}
              onDone={() => setSetup('done')}
            />
          </main>
        </div>
      </div>
    );
  }

  // A muted dot on a section whose feature is switched off, so the master
  // switch (which lives inside the section) is discoverable from the sidebar.
  // Prevent Sleep and General have no master switch, so they never carry one.
  const featureOff: Record<Section, boolean> = {
    keyboard: settings ? !settings.keyboardEnabled : false,
    window: settings ? !settings.windowManagementEnabled : false,
    menubar: settings ? !settings.menuBarTidyEnabled : false,
    session: false,
    general: false,
  };

  return (
    <div className="app">
      <nav className="sidebar" aria-label={t('app.sections')}>
        {SECTIONS.map((id) => (
          <button
            key={id}
            type="button"
            className={`nav-item ${section === id ? 'nav-item--active' : ''}`}
            aria-current={section === id ? 'true' : undefined}
            // When off, fold the state into the accessible name; the dot itself
            // is decorative.
            aria-label={
              featureOff[id] ? `${t(`app.nav.${id}`)} (${t('app.featureOff')})` : undefined
            }
            onClick={() => setSection(id)}
          >
            <SectionIcon name={id} />
            {t(`app.nav.${id}`)}
            {featureOff[id] && <span className="nav-item__dot" aria-hidden="true" />}
          </button>
        ))}
      </nav>

      <div className="app__content">
        {setup === 'dismissed' && (
          <div className="setup-banner">
            <span>{t('setup.bannerText')}</span>
            <button type="button" className="btn btn--ghost" onClick={openSetup}>
              {t('setup.bannerAction')}
            </button>
          </div>
        )}

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
              {section === 'keyboard' && <KeyboardView onOpenSetup={openSetup} />}
              {section === 'window' && <WindowView onOpenSetup={openSetup} />}
              {section === 'menubar' && <MenuBarView />}
              {section === 'session' && <SessionView />}
              {section === 'general' && (
                <GeneralView
                  autoCheckUpdate={autoCheckUpdate}
                  onAutoCheckHandled={onAutoCheckHandled}
                />
              )}
            </>
          )}
        </main>
      </div>
    </div>
  );
}
