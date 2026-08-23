import { listen } from '@tauri-apps/api/event';
import { useCallback, useEffect, useRef, useState } from 'react';

import { BrandIcon, SectionIcon, type SectionName } from './components/icons';
import { Banner, PermissionStatus } from './components/ui';
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

const TOOL_SECTIONS = ['window', 'keyboard', 'menubar', 'session'] as const satisfies Section[];
const APP_SECTIONS = ['general'] as const satisfies Section[];
const SECTIONS = [...TOOL_SECTIONS, ...APP_SECTIONS] as const;
const LAST_SECTION_KEY = 'tomari.settings.lastSection';

function readLastSection(): Section {
  try {
    const saved = window.localStorage.getItem(LAST_SECTION_KEY);
    return SECTIONS.includes(saved as Section) ? (saved as Section) : 'window';
  } catch {
    return 'window';
  }
}

export function App() {
  return (
    <SettingsProvider>
      <Localized />
    </SettingsProvider>
  );
}

function Localized() {
  const { settings } = useSettings();
  const lang = resolveLang(settings?.language ?? 'system');

  useEffect(() => {
    document.documentElement.lang = lang;
  }, [lang]);

  return (
    <I18nProvider lang={lang}>
      <AppShell />
    </I18nProvider>
  );
}

function AppShell() {
  const t = useT();
  const { settings, loadError, retryLoad, saveError, applyWarnings } = useSettings();
  const [section, setSection] = useState<Section>(readLastSection);
  const [autoCheckUpdate, setAutoCheckUpdate] = useState(false);
  const [setupLoaded, setSetupLoaded] = useState(false);
  const [setupOpen, setSetupOpen] = useState(false);
  const [updateRegrant, setUpdateRegrant] = useState(false);
  const [permissions, setPermissions] = useState<SetupPermissions>({
    accessibility: true,
    inputMonitoring: true,
  });
  const mainRef = useRef<HTMLElement>(null);

  useEffect(() => {
    try {
      window.localStorage.setItem(LAST_SECTION_KEY, section);
    } catch {
      // A disabled storage area should not prevent navigation.
    }
  }, [section]);

  useEffect(() => {
    const unlisten = listen('tomari:check-update', () => {
      setSection('general');
      setAutoCheckUpdate(true);
    });
    return () => void unlisten.then((fn) => fn());
  }, []);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const status = await api.setupStatus();
        if (cancelled) return;
        const nextPermissions = {
          accessibility: status.accessibility,
          inputMonitoring: status.inputMonitoring,
        };
        const missing = !nextPermissions.accessibility || !nextPermissions.inputMonitoring;
        setPermissions(nextPermissions);
        setUpdateRegrant(status.updateRegrant);
        setSetupOpen(missing && (status.firstRun || status.updateRegrant));
      } catch {
        // Settings remain usable if the optional setup status pull fails.
      } finally {
        if (!cancelled) setSetupLoaded(true);
      }
    })();

    const unlisten = listen<PermissionsChanged>('tomari:permissions-changed', (event) =>
      setPermissions({
        accessibility: event.payload.accessibility,
        inputMonitoring: event.payload.inputMonitoring,
      }),
    );
    return () => {
      cancelled = true;
      void unlisten.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    if (mainRef.current) mainRef.current.scrollTop = 0;
  }, [section]);

  const onAutoCheckHandled = useCallback(() => setAutoCheckUpdate(false), []);
  const openSetup = useCallback(() => setSetupOpen(true), []);
  const closeSetup = useCallback(() => {
    setSetupOpen(false);
    setUpdateRegrant(false);
  }, []);
  const onGranted = useCallback(
    (patch: Partial<SetupPermissions>) => setPermissions((current) => ({ ...current, ...patch })),
    [],
  );
  const permissionsReady = permissions.accessibility && permissions.inputMonitoring;

  if (!setupLoaded) {
    return (
      <div className="app app--loading">
        <span className="loading-mark" aria-hidden="true" />
        <span>{t('common.loading')}</span>
      </div>
    );
  }

  return (
    <div className="app">
      <nav className="sidebar" aria-label={t('app.sections')}>
        <div className="brand" aria-label="Tomari">
          <BrandIcon />
          <strong>Tomari</strong>
        </div>

        <SidebarGroup
          label={t('app.tools')}
          sections={TOOL_SECTIONS}
          selected={section}
          onSelect={setSection}
        />
        <SidebarGroup
          label={t('app.app')}
          sections={APP_SECTIONS}
          selected={section}
          onSelect={setSection}
        />

        <div className="sidebar__footer">
          <PermissionStatus
            ready={permissionsReady}
            readyLabel={t('app.permissionsReady')}
            attentionLabel={t('app.permissionsAttention')}
            onClick={openSetup}
          />
        </div>
      </nav>

      <div className="app__content">
        {saveError !== null && (
          <p className="alert" role="alert">
            {t('settings.saveFailed', { error: formatCmdError(saveError, t) })}
          </p>
        )}

        {applyWarnings.length > 0 && section !== 'general' && (
          <output className="apply-banner">
            <span>
              <strong>{t('settings.applyWarningTitle')}</strong>
              <small>{t('settings.applyWarningShell')}</small>
            </span>
            <button type="button" className="btn btn--ghost" onClick={() => setSection('general')}>
              {t('settings.reviewWarning')}
            </button>
          </output>
        )}

        <main ref={mainRef} className="app__main" aria-label={t(`app.nav.${section}`)}>
          {settings === null && loadError !== null ? (
            <Banner tone="warn">
              <div className="banner__body" role="alert">
                <p>{t('common.loadFailed', { error: formatCmdError(loadError, t) })}</p>
              </div>
              <button type="button" className="btn btn--primary" onClick={retryLoad}>
                {t('common.retry')}
              </button>
            </Banner>
          ) : (
            <SelectedView
              section={section}
              autoCheckUpdate={autoCheckUpdate}
              onAutoCheckHandled={onAutoCheckHandled}
              onOpenKeyboard={() => setSection('keyboard')}
            />
          )}
        </main>
      </div>

      {setupOpen && (
        <SetupDialog
          label={t('setup.title')}
          permissions={permissions}
          updateRegrant={updateRegrant}
          onGranted={onGranted}
          onDismiss={closeSetup}
        />
      )}
    </div>
  );
}

function SetupDialog({
  label,
  permissions,
  updateRegrant,
  onGranted,
  onDismiss,
}: {
  label: string;
  permissions: SetupPermissions;
  updateRegrant: boolean;
  onGranted: (patch: Partial<SetupPermissions>) => void;
  onDismiss: () => void;
}) {
  const dialogRef = useRef<HTMLDialogElement>(null);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    if (typeof dialog.showModal === 'function') {
      dialog.showModal();
    } else {
      dialog.setAttribute('open', '');
    }

    return () => {
      if (typeof dialog.close === 'function' && dialog.open) {
        dialog.close();
      } else {
        dialog.removeAttribute('open');
      }
    };
  }, []);

  return (
    <dialog
      ref={dialogRef}
      className="setup-dialog"
      aria-label={label}
      onCancel={(event) => {
        event.preventDefault();
        onDismiss();
      }}
    >
      <SetupView
        permissions={permissions}
        updateRegrant={updateRegrant}
        onGranted={onGranted}
        onDismiss={onDismiss}
        onDone={onDismiss}
      />
    </dialog>
  );
}

function SidebarGroup({
  label,
  sections,
  selected,
  onSelect,
}: {
  label: string;
  sections: readonly Section[];
  selected: Section;
  onSelect: (section: Section) => void;
}) {
  const t = useT();
  return (
    <section className="sidebar__group" aria-label={label}>
      <h2>{label}</h2>
      {sections.map((id) => (
        <button
          key={id}
          type="button"
          className={`nav-item ${selected === id ? 'nav-item--active' : ''}`}
          aria-current={selected === id ? 'page' : undefined}
          onClick={() => onSelect(id)}
        >
          <SectionIcon name={id} />
          <span>{t(`app.nav.${id}`)}</span>
        </button>
      ))}
    </section>
  );
}

function SelectedView({
  section,
  autoCheckUpdate,
  onAutoCheckHandled,
  onOpenKeyboard,
}: {
  section: Section;
  autoCheckUpdate: boolean;
  onAutoCheckHandled: () => void;
  onOpenKeyboard: () => void;
}) {
  if (section === 'keyboard') return <KeyboardView />;
  if (section === 'window') {
    return <WindowView onOpenKeyboard={onOpenKeyboard} />;
  }
  if (section === 'menubar') return <MenuBarView />;
  if (section === 'session') return <SessionView />;
  return <GeneralView autoCheckUpdate={autoCheckUpdate} onAutoCheckHandled={onAutoCheckHandled} />;
}
