import { getVersion } from '@tauri-apps/api/app';
import { useEffect, useRef, useState } from 'react';

import { Banner, FeaturePageHeader, SettingsList, SettingsRow, SwitchRow } from '../components/ui';
import * as api from '../lib/api';
import { applyWarningText } from '../lib/applyWarnings';
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
  const { settings, applyWarnings, update } = useSettings();
  const [version, setVersion] = useState('');
  const [updateStatus, setUpdateStatus] = useState<UpdateState>({ phase: 'idle' });
  // Turning the menu bar icon off hides the only visible affordance of an
  // Accessory app (no Dock icon either), so confirm it first and spell out the
  // ways back in. Turning it back on needs no confirmation.
  const [confirmHideTray, setConfirmHideTray] = useState(false);
  // Guards against overlapping checks: the tray entry (via StrictMode's double
  // mount, or rapid clicks) and the manual button share one in-flight check.
  const checking = useRef(false);
  const hideTrayConfirmRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    void getVersion().then(setVersion);
  }, []);

  // Move focus to the banner's primary action as soon as it appears, so a
  // keyboard user lands directly on the confirm/cancel controls instead of
  // having to hunt for them; Escape below gives a matching way to back out.
  useEffect(() => {
    if (confirmHideTray) hideTrayConfirmRef.current?.focus();
  }, [confirmHideTray]);

  // The tray's "Check for Updates" entry opens this panel and asks it to run
  // the check, so the result shows up here. The `checking` ref serializes
  // runs, so a setter resolving after `await` can never be a stale overlap.
  // oxlint-disable-next-line react-doctor/no-set-state-after-await-in-effect
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

  const maintenanceDesc = updateDesc(updateStatus, t);

  return (
    <div className="view">
      <FeaturePageHeader
        title={t('settings.general')}
        description={t('settings.pageDescription')}
      />

      <SettingsList label={t('settings.startup')}>
        <SwitchRow
          title={t('settings.launchAtLogin')}
          checked={settings.launchAtLogin}
          onChange={(v) => update({ launchAtLogin: v })}
        />
        <SwitchRow
          title={t('settings.showInMenuBar')}
          desc={!settings.showInMenuBar ? t('settings.hiddenHint') : undefined}
          checked={settings.showInMenuBar}
          onChange={(v) => {
            if (v) {
              update({ showInMenuBar: true });
            } else {
              setConfirmHideTray(true);
            }
          }}
        />
        <SettingsRow
          title={t('settings.language')}
          trail={
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
          }
        />
      </SettingsList>

      {applyWarnings.length > 0 && (
        <Banner tone="warn">
          <div className="banner__body">
            <strong>{t('settings.applyWarningTitle')}</strong>
            {applyWarnings.map((code) => (
              <p key={code}>{applyWarningText(code, t)}</p>
            ))}
          </div>
        </Banner>
      )}

      {confirmHideTray && (
        <Banner tone="warn">
          <div className="banner__body">
            <strong>{t('settings.hideTrayConfirmTitle')}</strong>
            <p>{t('settings.hideTrayConfirmBody')}</p>
            <div className="banner__actions">
              <button
                ref={hideTrayConfirmRef}
                type="button"
                className="btn btn--amber"
                onClick={() => {
                  update({ showInMenuBar: false });
                  setConfirmHideTray(false);
                }}
                onKeyDown={(e) => {
                  if (e.key === 'Escape') setConfirmHideTray(false);
                }}
              >
                {t('settings.hideTrayConfirmAction')}
              </button>
              <button
                type="button"
                className="btn btn--ghost"
                onClick={() => setConfirmHideTray(false)}
                onKeyDown={(e) => {
                  if (e.key === 'Escape') setConfirmHideTray(false);
                }}
              >
                {t('common.cancel')}
              </button>
            </div>
          </div>
        </Banner>
      )}

      <SettingsList label={t('settings.externalControl')}>
        <SwitchRow
          title={t('settings.externalWindowActions')}
          desc={t('settings.externalControlHint')}
          checked={settings.externalWindowActionsEnabled}
          onChange={(v) => update({ externalWindowActionsEnabled: v })}
        />
      </SettingsList>

      <SettingsList label={t('settings.maintenance')}>
        <SettingsRow
          title={`${t('settings.version')} ${version}`}
          description={maintenanceDesc}
          trail={
            updateStatus.phase === 'available' || updateStatus.phase === 'installing' ? (
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
            )
          }
        />
      </SettingsList>
    </div>
  );
}

// Map a save_settings apply-warning code to its localized message, falling back
// to a generic line for any code this build doesn't recognize.

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
