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
      <SettingsRoot />
    </I18nProvider>
  );
}

// Do not mount permission listeners or any feature view until the settings
// snapshot is known healthy. Recovery is a process-wide safety state, not a
// warning layered over controls that could continue issuing commands.
function SettingsRoot() {
  const t = useT();
  const { settings, settingsRecovery, loadError, retryLoad } = useSettings();

  if (settingsRecovery !== null) return <SettingsRecoveryView />;
  if (settings === null && loadError === null) {
    return (
      <output className="loading-shell">
        <span className="loading-mark" aria-hidden="true" />
        <span>{t('common.loading')}</span>
      </output>
    );
  }
  if (settings === null) {
    return (
      <div className="recovery-shell recovery-shell--load-error">
        <main className="recovery-card" aria-label={t('common.loadFailedTitle')}>
          <div className="recovery-brand" aria-label="Tomari">
            <BrandIcon />
            <strong>Tomari</strong>
          </div>
          <Banner tone="warn">
            <div className="banner__body" role="alert">
              <strong>{t('common.loadFailedTitle')}</strong>
              <p>{t('common.loadFailed', { error: formatCmdError(loadError, t) })}</p>
            </div>
            <button type="button" className="btn btn--primary" onClick={retryLoad}>
              {t('common.retry')}
            </button>
          </Banner>
        </main>
      </div>
    );
  }
  return <OperationalShell />;
}

function OperationalShell() {
  const t = useT();
  const { saveError, applyWarnings, configurationWarnings } = useSettings();
  const [section, setSection] = useState<Section>(readLastSection);
  const [configurationFocusRequest, setConfigurationFocusRequest] = useState(0);
  const [autoCheckUpdate, setAutoCheckUpdate] = useState(false);
  const [setupLoaded, setSetupLoaded] = useState(false);
  const [setupOpen, setSetupOpen] = useState(false);
  const [updateRegrant, setUpdateRegrant] = useState(false);
  // Fail closed: until a snapshot has arrived the permissions are *unknown*,
  // never assumed granted — the status shows "checking", not "ready".
  const [permissions, setPermissions] = useState<SetupPermissions>({
    accessibility: false,
    inputMonitoring: false,
  });
  const [permissionsKnown, setPermissionsKnown] = useState(false);
  const [setupAttempt, setSetupAttempt] = useState(0);
  // Set once the `tomari:permissions-changed` listener is registered; the
  // status pull waits for it, so no transition can land unobserved between
  // the two.
  const [permissionListenerReady, setPermissionListenerReady] = useState(false);
  // Revision of the newest permission snapshot applied, from either the event
  // stream or the pull, and that snapshot's values. A snapshot whose revision
  // is not strictly newer is discarded: the event and a pull can carry the
  // same revision while reading the bits at different moments, and letting
  // the later arrival win would let state run backwards.
  const permissionRevisionRef = useRef(-1);
  const permissionsRef = useRef<SetupPermissions>({ accessibility: false, inputMonitoring: false });
  const mainRef = useRef<HTMLElement>(null);

  // Apply a snapshot if it is newer than what is shown; returns the effective
  // permissions afterwards (the winner, whichever that is).
  const applyPermissionSnapshot = useCallback(
    (snapshot: {
      accessibility: boolean;
      inputMonitoring: boolean;
      revision: number;
    }): SetupPermissions => {
      if (snapshot.revision > permissionRevisionRef.current) {
        permissionRevisionRef.current = snapshot.revision;
        permissionsRef.current = {
          accessibility: snapshot.accessibility,
          inputMonitoring: snapshot.inputMonitoring,
        };
        setPermissions(permissionsRef.current);
        setPermissionsKnown(true);
      }
      return permissionsRef.current;
    },
    [],
  );

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
    return () => void unlisten.then((fn) => fn()).catch(() => {});
  }, []);

  // Subscribe to permission transitions for the app's lifetime. Registered
  // once and awaited, so the pull below cannot start until the listener is
  // actually in place — a transition in that window would otherwise be lost.
  useEffect(() => {
    let cancelled = false;
    const unlisten = listen<PermissionsChanged>('tomari:permissions-changed', (event) => {
      applyPermissionSnapshot(event.payload);
    });
    void unlisten
      .then(() => {
        if (!cancelled) setPermissionListenerReady(true);
        return null;
      })
      .catch(() => {
        // Registration failed: transitions will not be observed this session,
        // but the app must still load and read the status once — the status
        // control's retry re-pulls. Better a status that can go stale than a
        // window stuck on Loading.
        if (!cancelled) setPermissionListenerReady(true);
      });
    return () => {
      cancelled = true;
      // Registration may have rejected; the unsubscribe must not re-raise it.
      void unlisten.then((fn) => fn()).catch(() => {});
    };
  }, [applyPermissionSnapshot]);

  // The status pull: after the listener is ready, and again on every retry.
  useEffect(() => {
    if (!permissionListenerReady) return;
    let cancelled = false;
    void (async () => {
      try {
        const status = await api.setupStatus();
        if (cancelled) return;
        // Applied only if no newer event has landed meanwhile; the setup
        // dialog is decided from the effective snapshot, whichever won.
        const effective = applyPermissionSnapshot(status);
        const missing = !effective.accessibility || !effective.inputMonitoring;
        // The update-regrant explanation is about permissions that are
        // missing *now*; if a newer snapshot says they are all back, that
        // context is over and must not resurface on a later, unrelated revoke.
        setUpdateRegrant(status.updateRegrant && missing);
        setSetupOpen(missing && (status.firstRun || status.updateRegrant));
      } catch {
        // The status stays unknown — shown as such, never as ready — and the
        // status control offers a retry. Settings remain usable meanwhile.
      } finally {
        if (!cancelled) setSetupLoaded(true);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [applyPermissionSnapshot, permissionListenerReady, setupAttempt]);

  useEffect(() => {
    if (mainRef.current) mainRef.current.scrollTop = 0;
  }, [section]);

  useEffect(() => {
    if (section !== 'keyboard' || configurationFocusRequest === 0) return;
    mainRef.current?.querySelector<HTMLElement>('#keyboard-configuration-issues-title')?.focus();
  }, [configurationFocusRequest, section]);

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
  const permissionState: 'ready' | 'attention' | 'unknown' = !permissionsKnown
    ? 'unknown'
    : permissionsReady
      ? 'ready'
      : 'attention';
  const retrySetupStatus = useCallback(() => setSetupAttempt((n) => n + 1), []);
  const reviewConfigurationWarnings = useCallback(() => {
    setSection('keyboard');
    setConfigurationFocusRequest((request) => request + 1);
  }, []);
  const configurationWarningCount =
    (configurationWarnings?.invalidHotkeys.length ?? 0) +
    (configurationWarnings?.invalidModifierRules.length ?? 0);

  if (!setupLoaded) {
    return (
      <output className="app app--loading">
        <span className="loading-mark" aria-hidden="true" />
        <span>{t('common.loading')}</span>
      </output>
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
            state={permissionState}
            readyLabel={t('app.permissionsReady')}
            attentionLabel={t('app.permissionsAttention')}
            unknownLabel={t('app.permissionsUnknown')}
            onClick={permissionState === 'unknown' ? retrySetupStatus : openSetup}
          />
        </div>
      </nav>

      <div className="app__content">
        {saveError !== null && (
          <p className="alert" role="alert">
            {t('settings.saveFailed', { error: formatCmdError(saveError, t) })}
          </p>
        )}

        {configurationWarningCount > 0 && (
          <aside className="configuration-warning-banner">
            <output
              className="configuration-warning-banner__copy"
              aria-live="polite"
              aria-atomic="true"
            >
              <strong>
                {t('keyboard.configurationWarningTitle', {
                  count: configurationWarningCount,
                })}
              </strong>
              <small>{t('keyboard.configurationWarningBody')}</small>
            </output>
            <button type="button" className="btn btn--ghost" onClick={reviewConfigurationWarnings}>
              {t('keyboard.configurationWarningAction')}
            </button>
          </aside>
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
          <SelectedView
            section={section}
            autoCheckUpdate={autoCheckUpdate}
            onAutoCheckHandled={onAutoCheckHandled}
            onOpenKeyboard={() => setSection('keyboard')}
          />
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

function SettingsRecoveryView() {
  const t = useT();
  const { settingsRecovery, retrySettingsRecovery, resetSettingsRecovery } = useSettings();
  const [confirmReset, setConfirmReset] = useState(false);
  const titleRef = useRef<HTMLHeadingElement>(null);
  const resetButtonRef = useRef<HTMLButtonElement>(null);
  const confirmButtonRef = useRef<HTMLButtonElement>(null);
  const busy = settingsRecovery?.phase === 'retrying' || settingsRecovery?.phase === 'resetting';

  useEffect(() => {
    titleRef.current?.focus();
  }, []);

  useEffect(() => {
    if (confirmReset) confirmButtonRef.current?.focus();
  }, [confirmReset]);

  useEffect(() => {
    if (!confirmReset || busy) return;
    const handleEscape = (event: globalThis.KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      setConfirmReset(false);
      resetButtonRef.current?.focus();
    };
    document.addEventListener('keydown', handleEscape);
    return () => document.removeEventListener('keydown', handleEscape);
  }, [busy, confirmReset]);

  if (settingsRecovery === null) return null;
  const databaseReset = settingsRecovery.kind === 'databaseReset';
  const error =
    settingsRecovery.phase === 'failed'
      ? t(settingsRecovery.action === 'retry' ? 'recovery.retryFailed' : 'recovery.resetFailed', {
          error: formatCmdError(settingsRecovery.error, t),
        })
      : null;

  function cancelReset() {
    if (busy) return;
    setConfirmReset(false);
    resetButtonRef.current?.focus();
  }

  return (
    <div className="recovery-shell" aria-busy={busy}>
      <main className="recovery-card" aria-labelledby="settings-recovery-title">
        <div className="recovery-brand" aria-label="Tomari">
          <BrandIcon />
          <strong>Tomari</strong>
        </div>

        <header className="recovery-header">
          <span className="recovery-eyebrow">{t('recovery.eyebrow')}</span>
          <h1 id="settings-recovery-title" ref={titleRef} tabIndex={-1}>
            {t('recovery.title')}
          </h1>
          <p>{t(databaseReset ? 'recovery.databaseResetIntro' : 'recovery.intro')}</p>
        </header>

        <div className="recovery-interlock" role="alert">
          <span className="interlock-mark" aria-hidden="true">
            <span className="interlock-mark__lead" />
            <span className="interlock-mark__arm" />
            <span className="interlock-mark__contact" />
          </span>
          <span className="recovery-interlock__copy">
            <strong>{t('recovery.pausedTitle')}</strong>
            <span>{t('recovery.pausedBody')}</span>
          </span>
        </div>

        <section className="recovery-actions" aria-labelledby="recovery-options-title">
          <div>
            <h2 id="recovery-options-title">
              {t(databaseReset ? 'recovery.databaseResetOptionsTitle' : 'recovery.optionsTitle')}
            </h2>
            <p>{t(databaseReset ? 'recovery.databaseResetOptionsBody' : 'recovery.optionsBody')}</p>
          </div>
          <div className="recovery-actions__buttons">
            {!databaseReset && (
              <button
                type="button"
                className="btn btn--primary"
                disabled={busy}
                onClick={() => {
                  setConfirmReset(false);
                  void retrySettingsRecovery();
                }}
              >
                {settingsRecovery.phase === 'retrying'
                  ? t('recovery.retrying')
                  : t('recovery.retry')}
              </button>
            )}
            <button
              ref={resetButtonRef}
              type="button"
              className={databaseReset ? 'btn btn--primary' : 'btn btn--ghost'}
              disabled={busy}
              onClick={() => setConfirmReset(true)}
            >
              {t('recovery.reset')}
            </button>
          </div>
        </section>

        {confirmReset && (
          <fieldset className="recovery-confirm" aria-describedby="recovery-confirm-description">
            <legend>{t('recovery.confirmTitle')}</legend>
            <p id="recovery-confirm-description">{t('recovery.confirmBody')}</p>
            <div className="recovery-confirm__buttons">
              <button
                ref={confirmButtonRef}
                type="button"
                className="btn btn--amber"
                disabled={busy}
                onClick={() => void resetSettingsRecovery()}
              >
                {settingsRecovery.phase === 'resetting'
                  ? t('recovery.resetting')
                  : t('recovery.confirmAction')}
              </button>
              <button
                type="button"
                className="btn btn--ghost"
                disabled={busy}
                onClick={cancelReset}
              >
                {t('common.cancel')}
              </button>
            </div>
          </fieldset>
        )}

        {error !== null && (
          <p className="recovery-error" role="alert">
            {error}
          </p>
        )}
      </main>
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
