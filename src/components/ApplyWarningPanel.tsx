import { applyWarningText } from '../lib/applyWarnings';
import { useT } from '../lib/i18n';
import { useRestartApp } from '../lib/restart';
import { useSettings } from '../lib/settings';

const RESTARTABLE_WARNINGS = new Set([
  'keyboardTap',
  'dragToSnapTap',
  'dragToMoveTap',
  'capsLockRemap',
  'commandImeRules',
  'menuBar',
]);

export function ApplyWarningPanel({
  permissions,
  onOpenPermissions,
  onOpenDiagnostics,
}: {
  permissions: { accessibility: boolean; inputMonitoring: boolean } | null;
  onOpenPermissions: () => void;
  onOpenDiagnostics: () => void;
}) {
  const t = useT();
  const { applyWarnings } = useSettings();
  const { restart, restarting, error, busy } = useRestartApp();
  const warnings = [...new Set(applyWarnings)];
  // Only monitoring failures depend on these grants. Shortcut registration,
  // rule synchronization, and HID mapping failures need their own recovery.
  const needsPermissions =
    permissions !== null &&
    warnings.some((code) => {
      if (code === 'keyboardTap' || code === 'dragToMoveTap') {
        return !permissions.accessibility || !permissions.inputMonitoring;
      }
      return code === 'dragToSnapTap' && !permissions.inputMonitoring;
    });
  const canRestart = !needsPermissions && warnings.some((code) => RESTARTABLE_WARNINGS.has(code));
  if (warnings.length === 0 && !restarting && error === null) return null;

  return (
    <aside className="apply-banner">
      <div className="apply-banner__body">
        <output aria-live="polite" aria-atomic="true">
          {warnings.map((code) => (
            <p key={code}>{applyWarningText(code, t)}</p>
          ))}
          {canRestart && <p>{t('app.restartHint')}</p>}
        </output>
        {error && <p role="alert">{error}</p>}
        <div className="banner__actions">
          {needsPermissions && (
            <button
              type="button"
              className="btn btn--primary"
              disabled={busy}
              onClick={onOpenPermissions}
            >
              {t('settings.openPermissionSettings')}
            </button>
          )}
          {(canRestart || restarting) && (
            <button
              type="button"
              className="btn btn--primary"
              disabled={busy}
              onClick={() => void restart()}
            >
              {t(restarting ? 'app.restarting' : 'app.restart')}
            </button>
          )}
          <button
            type="button"
            className={`btn ${needsPermissions || canRestart ? 'btn--ghost' : 'btn--primary'}`}
            disabled={busy}
            onClick={onOpenDiagnostics}
          >
            {t('settings.openDiagnostics')}
          </button>
        </div>
      </div>
    </aside>
  );
}
