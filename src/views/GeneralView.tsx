import { getVersion } from '@tauri-apps/api/app';
import { useEffect, useRef, useState } from 'react';

import { Banner, Group, SwitchRow } from '../components/ui';
import * as api from '../lib/api';
import { cmdErrorMessage } from '../lib/errors';
import { useT, type Translator } from '../lib/i18n';
import { useSettings } from '../lib/settings';
import type { Language, UpdateInfo } from '../lib/types';

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

export function GeneralView({
  autoCheckUpdate = false,
  onAutoCheckHandled,
}: {
  autoCheckUpdate?: boolean;
  onAutoCheckHandled?: () => void;
}) {
  const t = useT();
  const { settings, update } = useSettings();
  const [version, setVersion] = useState('');
  const [updateStatus, setUpdateStatus] = useState<UpdateState>({ phase: 'idle' });
  // Turning the menu bar icon off hides the only visible affordance of an
  // Accessory app (no Dock icon either), so confirm it first and spell out the
  // ways back in. Turning it back on needs no confirmation.
  const [confirmHideTray, setConfirmHideTray] = useState(false);
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
          onChange={(v) => {
            if (v) {
              update({ showInMenuBar: true });
            } else {
              setConfirmHideTray(true);
            }
          }}
        />
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

      {confirmHideTray && (
        <Banner tone="warn">
          <div className="banner__body">
            <strong>{t('settings.hideTrayConfirmTitle')}</strong>
            <p>{t('settings.hideTrayConfirmBody')}</p>
            <div className="banner__actions">
              <button
                type="button"
                className="btn btn--amber"
                onClick={() => {
                  update({ showInMenuBar: false });
                  setConfirmHideTray(false);
                }}
              >
                {t('settings.hideTrayConfirmAction')}
              </button>
              <button
                type="button"
                className="btn btn--ghost"
                onClick={() => setConfirmHideTray(false)}
              >
                {t('common.cancel')}
              </button>
            </div>
          </div>
        </Banner>
      )}

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
