import { getVersion } from '@tauri-apps/api/app';
import { listen } from '@tauri-apps/api/event';
import { useEffect, useRef, useState } from 'react';

import { Chip, Group, Segmented, SwitchRow } from '../components/ui';
import * as api from '../lib/api';
import { cmdErrorMessage, formatCmdError } from '../lib/errors';
import { useT, type Translator } from '../lib/i18n';
import { useSettings } from '../lib/settings';
import type {
  ImportReport,
  KeepAwakeStatus,
  Language,
  LidCloseState,
  Theme,
  UpdateInfo,
} from '../lib/types';

const THEMES: Theme[] = ['system', 'light', 'dark'];

// Each language is listed in its own name, so it stays findable whatever the
// current UI language; only "System" follows the UI language.
const LANGUAGES: Language[] = ['system', 'en', 'ja'];
const LANGUAGE_NAMES: Record<Exclude<Language, 'system'>, string> = {
  en: 'English',
  ja: '日本語',
};

type UpdateState =
  | { phase: 'idle' }
  | { phase: 'checking' }
  | { phase: 'upToDate' }
  | { phase: 'available'; update: UpdateInfo; error?: string }
  | { phase: 'installing'; update: UpdateInfo }
  | { phase: 'error'; message: string };

export function SettingsView({
  autoCheckUpdate = false,
  onAutoCheckHandled,
}: {
  autoCheckUpdate?: boolean;
  onAutoCheckHandled?: () => void;
}) {
  const t = useT();
  const { settings, update, reload } = useSettings();
  const [version, setVersion] = useState('');
  const [updateStatus, setUpdateStatus] = useState<UpdateState>({ phase: 'idle' });
  // Guards against overlapping checks: the tray entry (via StrictMode's double
  // mount, or rapid clicks) and the manual button share one in-flight check.
  const checking = useRef(false);

  useEffect(() => {
    void getVersion().then(setVersion);
  }, []);

  // The tray's "Check for Updates" entry opens this panel and asks it to run
  // the check, so the result shows up here.
  useEffect(() => {
    if (!autoCheckUpdate) return;
    void checkForUpdate();
    onAutoCheckHandled?.();
  }, [autoCheckUpdate, onAutoCheckHandled]);

  async function checkForUpdate() {
    if (checking.current) return;
    checking.current = true;
    setUpdateStatus({ phase: 'checking' });
    try {
      const found = await api.checkForUpdate();
      setUpdateStatus(found ? { phase: 'available', update: found } : { phase: 'upToDate' });
    } catch (e) {
      // Update-check failures are always uncategorized (updater/network), so
      // show the message directly and keep `t` out of this effect-run path.
      setUpdateStatus({ phase: 'error', message: cmdErrorMessage(e) });
    } finally {
      checking.current = false;
    }
  }

  async function installUpdate(info: UpdateInfo) {
    setUpdateStatus({ phase: 'installing', update: info });
    try {
      // On success the app relaunches, so this never resolves.
      await api.installUpdate();
    } catch (e) {
      // The backend keeps the pending update, so offer the install again.
      setUpdateStatus({ phase: 'available', update: info, error: cmdErrorMessage(e) });
    }
  }

  if (!settings) return <div className="view">{t('common.loading')}</div>;

  return (
    <div className="view">
      <KeepAwakeSection t={t} />

      <Group label={t('settings.general')}>
        <SwitchRow
          title={t('settings.launchAtLogin')}
          checked={settings.launchAtLogin}
          onChange={(v) => update({ launchAtLogin: v })}
        />
        <SwitchRow
          title={t('settings.showInMenuBar')}
          desc={settings.showInMenuBar ? undefined : t('settings.hiddenHint')}
          checked={settings.showInMenuBar}
          onChange={(v) => update({ showInMenuBar: v })}
        />
        <div className="item">
          <div className="item__body">
            <span className="item__title">{t('settings.appearance')}</span>
          </div>
          <div className="item__trail">
            <Segmented
              value={settings.theme}
              options={THEMES.map((theme) => ({
                value: theme,
                label: t(`settings.theme.${theme}`),
              }))}
              onChange={(v) => update({ theme: v })}
              ariaLabel={t('settings.appearance')}
            />
          </div>
        </div>
        <div className="item">
          <div className="item__body">
            <span className="item__title">{t('settings.language')}</span>
          </div>
          <div className="item__trail">
            <select
              className="input"
              value={settings.language}
              onChange={(e) => update({ language: e.target.value as Language })}
              aria-label={t('settings.language')}
            >
              {LANGUAGES.map((language) => (
                <option key={language} value={language}>
                  {language === 'system' ? t('settings.language.system') : LANGUAGE_NAMES[language]}
                </option>
              ))}
            </select>
          </div>
        </div>
      </Group>

      <Group label={t('settings.externalControl')} note={t('settings.externalControlHint')}>
        <SwitchRow
          title={t('settings.externalWindowActions')}
          checked={settings.externalWindowActionsEnabled}
          onChange={(v) => update({ externalWindowActionsEnabled: v })}
        />
      </Group>

      <Group label={t('settings.maintenance')}>
        <div className="item">
          <div className="item__body">
            <span className="item__title">
              {t('settings.version')} {version}
            </span>
            {updateDesc(updateStatus, t) && (
              <span className="item__desc">{updateDesc(updateStatus, t)}</span>
            )}
          </div>
          <div className="item__trail">
            {updateStatus.phase === 'available' || updateStatus.phase === 'installing' ? (
              <button
                type="button"
                className="btn btn--primary"
                disabled={updateStatus.phase === 'installing'}
                onClick={() => void installUpdate(updateStatus.update)}
              >
                {updateStatus.phase === 'installing'
                  ? t('settings.installing')
                  : t('settings.installRestart')}
              </button>
            ) : (
              <button
                type="button"
                className="btn"
                disabled={updateStatus.phase === 'checking'}
                onClick={() => void checkForUpdate()}
              >
                {updateStatus.phase === 'checking'
                  ? t('settings.checking')
                  : t('settings.checkUpdates')}
              </button>
            )}
          </div>
        </div>

        <BackupSection t={t} onImported={() => void reload()} />
      </Group>
    </div>
  );
}

function updateDesc(state: UpdateState, t: Translator): string | null {
  switch (state.phase) {
    case 'available':
      return (
        t('settings.updateAvailable', { version: state.update.version }) +
        (state.update.notes ? ` ${state.update.notes}` : '') +
        (state.error ? ` ${t('settings.updateFailed', { error: state.error })}` : '')
      );
    case 'installing':
      return t('settings.updateAvailable', { version: state.update.version });
    case 'upToDate':
      return t('settings.upToDate');
    case 'error':
      return t('settings.updateCheckFailed', { error: state.message });
    default:
      return null;
  }
}

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
// AppSettings: this section fetches it on open and follows the
// "tomari:keep-awake-changed" event so the tray and the panel stay in sync.
function KeepAwakeSection({ t }: { t: Translator }) {
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
    <Group label={t('settings.session')}>
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
            {lid === 'unavailable' && (
              <span className="item__desc">{t('settings.keepAwakeNoLidClose')}</span>
            )}
          </div>
          <div className="item__trail">
            <Chip tone={chip.tone}>{t(chip.key as Parameters<Translator>[0])}</Chip>
          </div>
        </div>
      )}
    </Group>
  );
}

type BackupState =
  | { phase: 'idle' }
  | { phase: 'working' }
  | { phase: 'confirmImport' }
  | { phase: 'exported'; path: string; omitted: number }
  | { phase: 'imported'; report: ImportReport }
  | { phase: 'rejected'; errors: string[] }
  | { phase: 'error'; message: string };

function BackupSection({ t, onImported }: { t: Translator; onImported: () => void }) {
  const [state, setState] = useState<BackupState>({ phase: 'idle' });
  const busy = state.phase === 'working';

  async function runExport() {
    setState({ phase: 'working' });
    try {
      const outcome = await api.exportConfig();
      setState(
        outcome.status === 'saved'
          ? { phase: 'exported', path: outcome.path, omitted: outcome.omitted }
          : { phase: 'idle' },
      );
    } catch (e) {
      setState({ phase: 'error', message: formatCmdError(e, t) });
    }
  }

  async function runImport() {
    setState({ phase: 'working' });
    try {
      const outcome = await api.importConfig();
      if (outcome.status === 'applied') {
        setState({ phase: 'imported', report: outcome.report });
        onImported();
      } else if (outcome.status === 'rejected') {
        setState({ phase: 'rejected', errors: outcome.errors });
      } else {
        setState({ phase: 'idle' });
      }
    } catch (e) {
      setState({ phase: 'error', message: formatCmdError(e, t) });
    }
  }

  return (
    <div className="item item--value">
      <span className="item__desc">{t('settings.backupHint')}</span>

      {state.phase === 'confirmImport' ? (
        <>
          <p className="hint hint--warn">{t('settings.importConfirm')}</p>
          <div className="row">
            <button type="button" className="btn btn--primary" onClick={() => void runImport()}>
              {t('settings.importContinue')}
            </button>
            <button type="button" className="btn" onClick={() => setState({ phase: 'idle' })}>
              {t('common.cancel')}
            </button>
          </div>
        </>
      ) : (
        <div className="row">
          <button type="button" className="btn" disabled={busy} onClick={() => void runExport()}>
            {busy ? t('settings.working') : t('settings.export')}
          </button>
          <button
            type="button"
            className="btn"
            disabled={busy}
            onClick={() => setState({ phase: 'confirmImport' })}
          >
            {t('settings.import')}
          </button>
        </div>
      )}

      {state.phase === 'exported' && (
        <>
          <p className="hint">{t('settings.exportSaved', { path: state.path })}</p>
          {state.omitted > 0 && (
            <p className="hint hint--warn">
              {t('settings.exportOmitted', { count: state.omitted })}
            </p>
          )}
        </>
      )}

      {state.phase === 'imported' && <ImportSummary t={t} report={state.report} />}

      {state.phase === 'rejected' && (
        <div className="hint hint--err">
          <p>{t('settings.importRejected')}</p>
          <ul className="backup__list">
            {state.errors.map((e) => (
              <li key={e}>{e}</li>
            ))}
          </ul>
        </div>
      )}

      {state.phase === 'error' && (
        <p className="hint hint--err">{t('settings.backupFailed', { error: state.message })}</p>
      )}
    </div>
  );
}

function ImportSummary({ t, report }: { t: Translator; report: ImportReport }) {
  return (
    <>
      <p className="hint">
        {t('settings.importApplied', {
          hotkeys: report.hotkeys,
          modifierRules: report.modifierRules,
        })}
      </p>
      {report.warnings.length > 0 && (
        <div className="hint">
          <p>{t('settings.importWarnings')}</p>
          <ul className="backup__list">
            {report.warnings.map((w) => (
              <li key={w}>{w}</li>
            ))}
          </ul>
        </div>
      )}
      {report.registrationFailures.length > 0 && (
        <div className="hint hint--warn">
          <p>{t('settings.importRegFailures')}</p>
          <ul className="backup__list">
            {report.registrationFailures.map((f) => (
              <li key={f}>{f}</li>
            ))}
          </ul>
        </div>
      )}
      <p className="hint">{t('settings.importBackedUp', { path: report.backupPath })}</p>
    </>
  );
}
