import { useCallback, useEffect, useId, useRef, useState } from 'react';

import * as api from '../lib/api';
import { formatCmdError, hasCmdErrorCode } from '../lib/errors';
import { useT, type MessageKey, type Translator } from '../lib/i18n';
import { useSettings } from '../lib/settings';
import type {
  AppSettings,
  ConfigurationBackupExport,
  ConfigurationBackupPreview,
} from '../lib/types';
import { SettingsList, SettingsRow } from './ui';

const MAX_BACKUP_BYTES = 2 * 1024 * 1024;
type Operation = 'idle' | 'exporting' | 'reading' | 'previewing' | 'applying' | 'revealing';

function useBackupOperations() {
  const t = useT();
  const { withSettingsSaved, applyConfigurationBackup } = useSettings();
  const [phase, setPhase] = useState<Operation>('idle');
  const [preview, setPreview] = useState<ConfigurationBackupPreview | null>(null);
  const [exported, setExported] = useState<ConfigurationBackupExport | null>(null);
  const [canRestore, setCanRestore] = useState<boolean | null>(null);
  const [statusError, setStatusError] = useState<unknown>(null);
  const [error, setError] = useState<unknown>(null);
  const [expired, setExpired] = useState(false);
  const [notice, setNotice] = useState<MessageKey | null>(null);
  const running = useRef(false);
  const generation = useRef(0);
  const statusGeneration = useRef(0);
  const contents = useRef<string | null>(null);

  const refreshStatus = useCallback(() => {
    const request = ++statusGeneration.current;
    return api.getConfigurationBackupStatus().then(
      (result) => {
        if (statusGeneration.current === request) {
          setCanRestore(result.canRestore);
          setStatusError(null);
        }
        return result;
      },
      (cause: unknown) => {
        if (statusGeneration.current === request) setStatusError(cause);
      },
    );
  }, []);

  useEffect(() => {
    void refreshStatus();
    return () => {
      generation.current += 1;
      statusGeneration.current += 1;
    };
  }, [refreshStatus]);

  async function run<T>(
    operation: Operation,
    task: (isCurrent: () => boolean) => Promise<T>,
    onSuccess: (value: T) => void,
  ) {
    if (running.current) return;
    running.current = true;
    const request = ++generation.current;
    setPhase(operation);
    setError(null);
    setNotice(null);
    try {
      const result = await task(() => generation.current === request);
      if (generation.current === request) onSuccess(result);
    } catch (cause) {
      if (generation.current === request) {
        setError(cause);
        if (hasCmdErrorCode(cause, 'backupPreviewExpired')) setExpired(true);
        else if (operation === 'applying') {
          setPreview(null);
          contents.current = null;
          setNotice('backup.applyUnconfirmed');
          void refreshStatus();
        }
      }
    } finally {
      if (generation.current === request) {
        running.current = false;
        setPhase('idle');
      }
    }
  }

  function adoptPreview(next: ConfigurationBackupPreview) {
    setPreview(next);
    setExpired(false);
  }

  function importFile(file: File) {
    void run(
      'reading',
      async (isCurrent) => {
        if (file.size > MAX_BACKUP_BYTES) throw new Error(t('backup.fileTooLarge'));
        const text = await file.text();
        if (!isCurrent()) throw new Error('Backup selection was cancelled.');
        contents.current = text;
        return withSettingsSaved(() => api.previewConfigurationImport(text));
      },
      adoptPreview,
    );
  }

  function reviewRestore() {
    contents.current = null;
    void run('previewing', () => withSettingsSaved(api.previewConfigurationRestore), adoptPreview);
  }

  function reviewAgain() {
    if (!preview) return;
    if (preview.kind === 'restore') reviewRestore();
    else if (contents.current !== null) {
      const text = contents.current;
      void run(
        'previewing',
        () => withSettingsSaved(() => api.previewConfigurationImport(text)),
        adoptPreview,
      );
    }
  }

  function apply() {
    if (!preview || expired) return;
    const restoring = preview.kind === 'restore';
    void run(
      'applying',
      () => applyConfigurationBackup(preview.token),
      () => {
        setPreview(null);
        contents.current = null;
        setCanRestore(!restoring);
        setNotice(restoring ? 'backup.restored' : 'backup.imported');
        void refreshStatus();
      },
    );
  }

  function cancel() {
    if (running.current) return;
    setPreview(null);
    contents.current = null;
    setError(null);
    setExpired(false);
  }

  return {
    phase,
    preview,
    exported,
    canRestore,
    statusError,
    error,
    expired,
    notice,
    refreshStatus,
    importFile,
    reviewRestore,
    reviewAgain,
    apply,
    cancel,
    exportSettings: () =>
      void run('exporting', () => withSettingsSaved(api.exportConfigurationBackup), setExported),
    revealFolder: () => void run('revealing', api.revealConfigurationBackups, () => {}),
  };
}

export function SettingsBackupPanel() {
  const t = useT();
  const backup = useBackupOperations();
  const fileRef = useRef<HTMLInputElement>(null);
  const importButtonRef = useRef<HTMLButtonElement>(null);
  const restoreButtonRef = useRef<HTMLButtonElement>(null);
  const previewWasOpen = useRef(false);
  const previewOrigin = useRef<'import' | 'restore'>('import');
  const busy = backup.phase !== 'idle';

  useEffect(() => {
    if (backup.preview !== null) {
      previewWasOpen.current = true;
      previewOrigin.current = backup.preview.kind;
    } else if (previewWasOpen.current && !busy) {
      previewWasOpen.current = false;
      const restoreButton = restoreButtonRef.current;
      const target =
        previewOrigin.current === 'restore' && restoreButton && !restoreButton.disabled
          ? restoreButton
          : importButtonRef.current;
      target?.focus();
    }
  }, [backup.preview, busy]);

  return (
    <>
      <SettingsList label={t('backup.title')} description={t('backup.privacy')}>
        <SettingsRow
          title={t('backup.exportTitle')}
          description={t('backup.exportDescription')}
          trail={
            <button type="button" className="btn" disabled={busy} onClick={backup.exportSettings}>
              {t(backup.phase === 'exporting' ? 'backup.exporting' : 'backup.export')}
            </button>
          }
        />
        <SettingsRow
          title={t('backup.importTitle')}
          description={t('backup.importDescription')}
          trail={
            <button
              ref={importButtonRef}
              type="button"
              className="btn"
              disabled={busy}
              onClick={(event) => {
                event.currentTarget.focus();
                fileRef.current?.click();
              }}
            >
              {t(backup.phase === 'reading' ? 'backup.checking' : 'backup.chooseFile')}
            </button>
          }
        />
        <input
          ref={fileRef}
          type="file"
          accept="application/json,.json"
          hidden
          aria-label={t('backup.chooseFile')}
          onChange={(event) => {
            const file = event.target.files?.[0];
            event.target.value = '';
            if (file) backup.importFile(file);
          }}
        />
        <SettingsRow
          title={t('backup.restoreTitle')}
          description={t(
            backup.canRestore === false ? 'backup.noRestore' : 'backup.restoreDescription',
          )}
          trail={
            <button
              ref={restoreButtonRef}
              type="button"
              className="btn btn--ghost"
              disabled={busy || backup.canRestore !== true}
              onClick={backup.reviewRestore}
            >
              {t(backup.phase === 'previewing' ? 'backup.checking' : 'backup.restore')}
            </button>
          }
        />
        {backup.statusError !== null && (
          <p className="backup-note hint--err" role="alert">
            {t('backup.statusFailed', { error: formatCmdError(backup.statusError, t) })}
            <button
              type="button"
              className="btn btn--ghost"
              disabled={busy}
              onClick={() => void backup.refreshStatus()}
            >
              {t('common.retry')}
            </button>
          </p>
        )}
        {backup.exported !== null && (
          <div className="backup-note">
            <output aria-live="polite">
              {t('backup.exported')} <span className="backup-path">{backup.exported.path}</span>
            </output>
            <button
              type="button"
              className="btn btn--ghost"
              disabled={busy}
              onClick={backup.revealFolder}
            >
              {t('backup.openFolder')}
            </button>
          </div>
        )}
        {backup.notice !== null && (
          <output className="backup-note" aria-live="polite">
            {t(backup.notice)}
          </output>
        )}
        {backup.error !== null && backup.preview === null && (
          <p className="backup-note hint--err" role="alert">
            {formatCmdError(backup.error, t)}
          </p>
        )}
      </SettingsList>
      {backup.preview !== null && (
        <BackupPreviewDialog
          preview={backup.preview}
          busy={busy}
          expired={backup.expired}
          error={backup.error}
          onCancel={backup.cancel}
          onApply={backup.apply}
          onReviewAgain={backup.reviewAgain}
        />
      )}
    </>
  );
}

const SETTING_LABELS: Record<keyof AppSettings, MessageKey> = {
  launchAtLogin: 'settings.launchAtLogin',
  language: 'settings.language',
  keyboardEnabled: 'settings.keyboardCustomization',
  windowManagementEnabled: 'settings.windowManagement',
  externalWindowActionsEnabled: 'settings.externalWindowActions',
  commandImeSwitchEnabled: 'keyboard.commandImeSwitch',
  showInMenuBar: 'settings.showInMenuBar',
  dragToSnapEnabled: 'backup.dragToSnap',
  dragToMoveEnabled: 'backup.dragToMove',
  menuBarTidyEnabled: 'app.nav.menubar',
  menuBarAutoCollapseSecs: 'menubar.autoCollapse',
};

function settingValue(value: AppSettings[keyof AppSettings], t: Translator): string {
  if (typeof value === 'boolean') return t(value ? 'common.on' : 'common.off');
  if (typeof value === 'number')
    return value === 0
      ? t('menubar.autoCollapseNever')
      : t('menubar.autoCollapseSecs', { secs: value });
  if (value === 'system') return t('settings.language.system');
  return value === 'ja' ? '日本語' : 'English';
}

const BACKUP_COUNTS = [
  ['hotkeys', 'backup.hotkeys'],
  ['modifierRules', 'backup.modifierRules'],
  ['windowPlacements', 'backup.windowPlacements'],
] as const;

function BackupPreviewDialog({
  preview,
  busy,
  expired,
  error,
  onCancel,
  onApply,
  onReviewAgain,
}: {
  preview: ConfigurationBackupPreview;
  busy: boolean;
  expired: boolean;
  error: unknown;
  onCancel: () => void;
  onApply: () => void;
  onReviewAgain: () => void;
}) {
  const t = useT();
  const titleId = useId();
  const descriptionId = useId();
  const dialogRef = useRef<HTMLDialogElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    dialog.showModal();
    cancelRef.current?.focus();
    return () => {
      if (dialog.open) dialog.close();
    };
  }, []);
  const changedSettings = (Object.keys(SETTING_LABELS) as Array<keyof AppSettings>).filter(
    (key) => preview.currentSettings[key] !== preview.settings[key],
  );

  return (
    <dialog
      ref={dialogRef}
      className="settings-sheet backup-preview"
      aria-labelledby={titleId}
      aria-describedby={descriptionId}
      onCancel={(event) => {
        event.preventDefault();
        if (!busy) onCancel();
      }}
    >
      <header className="settings-sheet__header">
        <h2 id={titleId}>
          {t(
            preview.kind === 'restore' ? 'backup.confirmRestoreTitle' : 'backup.confirmImportTitle',
          )}
        </h2>
      </header>
      <p id={descriptionId} className="hint">
        {t(preview.kind === 'restore' ? 'backup.confirmRestoreBody' : 'backup.confirmImportBody')}
      </p>
      <table className="backup-preview__table">
        <thead>
          <tr>
            <th scope="col">{t('backup.items')}</th>
            <th scope="col">{t('backup.current')}</th>
            <th scope="col">{t('backup.replacement')}</th>
          </tr>
        </thead>
        <tbody>
          {BACKUP_COUNTS.map(([key, label]) => (
            <tr key={key}>
              <th scope="row">{t(label)}</th>
              <td>{preview.current[key]}</td>
              <td>{preview.incoming[key]}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <h3>{t('backup.settingsChanges')}</h3>
      {changedSettings.length === 0 ? (
        <p className="hint">{t('backup.settingsUnchanged')}</p>
      ) : (
        <table className="backup-preview__table">
          <thead>
            <tr>
              <th scope="col">{t('backup.setting')}</th>
              <th scope="col">{t('backup.current')}</th>
              <th scope="col">{t('backup.replacement')}</th>
            </tr>
          </thead>
          <tbody>
            {changedSettings.map((key) => (
              <tr key={key}>
                <th scope="row">{t(SETTING_LABELS[key])}</th>
                <td>{settingValue(preview.currentSettings[key], t)}</td>
                <td>{settingValue(preview.settings[key], t)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <p className="hint">{t('backup.runtimeExcluded')}</p>
      {!preview.settings.showInMenuBar && <p className="hint">{t('backup.hiddenIcon')}</p>}
      {error !== null && (
        <p className="hint hint--err" role="alert">
          {formatCmdError(error, t)}
        </p>
      )}
      <footer className="backup-preview__actions">
        <button
          ref={cancelRef}
          type="button"
          className="btn btn--ghost"
          disabled={busy}
          onClick={onCancel}
        >
          {t('common.cancel')}
        </button>
        {expired ? (
          <button type="button" className="btn" disabled={busy} onClick={onReviewAgain}>
            {t('backup.reviewAgain')}
          </button>
        ) : (
          <button type="button" className="btn btn--amber" disabled={busy} onClick={onApply}>
            {t(busy ? 'backup.applying' : 'backup.replaceAll')}
          </button>
        )}
      </footer>
    </dialog>
  );
}
