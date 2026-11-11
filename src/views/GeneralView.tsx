import { getVersion } from '@tauri-apps/api/app';
import { useEffect, useRef, useState } from 'react';

import { SettingsBackupPanel } from '../components/SettingsBackupPanel';
import { Banner, FeaturePageHeader, SettingsList, SettingsRow, SwitchRow } from '../components/ui';
import { checkForUpdate, installUpdate, useUpdateState, type UpdateState } from '../lib/appUpdate';
import { useT, type Translator } from '../lib/i18n';
import { useSettings } from '../lib/settings';
import type { Language } from '../lib/types';

// Each language is listed in its own name, so it stays findable whatever the
// current UI language; only "System" follows the UI language.
const LANGUAGES: Language[] = ['system', 'en', 'ja'];
const LANGUAGE_NAMES: Record<Exclude<Language, 'system'>, string> = {
  en: 'English',
  ja: '日本語',
};

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
  // Check/install progress is app-wide state, so it survives leaving and
  // re-entering this view.
  const updateStatus = useUpdateState();
  // Turning the menu bar icon off hides the only visible affordance of an
  // Accessory app (no Dock icon either), so confirm it first and spell out the
  // ways back in. Turning it back on needs no confirmation.
  const [confirmHideTray, setConfirmHideTray] = useState(false);
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
  // the check, so the result shows up here. The shared update state drops the
  // request while a check or an install is already running.
  useEffect(() => {
    if (!autoCheckUpdate) return;
    void checkForUpdate();
    onAutoCheckHandled?.();
  }, [autoCheckUpdate, onAutoCheckHandled]);

  if (!settings) return <div className="view">{t('common.loading')}</div>;

  const maintenanceDesc = updateDesc(updateStatus, t);

  return (
    <div className="view">
      <FeaturePageHeader title={t('settings.general')} />

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

      <SettingsBackupPanel />

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
                onClick={() => void installUpdate()}
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
