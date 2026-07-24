// The permission-setup checklist, shown in place of the tabs while a
// permission is missing. Permission state lives in App (fed by the initial
// setup_status pull and "tomari:permissions-changed"); this view only renders
// it and forwards grant requests, reporting an immediate grant back up so the
// row flips without waiting for the backend's next poll tick.

import { useState } from 'react';

import { Chip, Group } from '../components/ui';
import * as api from '../lib/api';
import { formatCmdError } from '../lib/errors';
import { useT } from '../lib/i18n';

export interface SetupPermissions {
  accessibility: boolean;
  inputMonitoring: boolean;
}

export function SetupView({
  permissions,
  updateRegrant,
  onGranted,
  onDismiss,
  onDone,
}: {
  permissions: SetupPermissions;
  // Show the "an update reset these permissions" explanation.
  updateRegrant: boolean;
  onGranted: (patch: Partial<SetupPermissions>) => void;
  onDismiss: () => void;
  onDone: () => void;
}) {
  const t = useT();
  const [error, setError] = useState<string | null>(null);
  const allGranted = permissions.accessibility && permissions.inputMonitoring;

  async function request(key: keyof SetupPermissions, call: () => Promise<boolean>) {
    try {
      const ok = await call();
      if (ok) onGranted({ [key]: true });
      setError(null);
    } catch (e) {
      setError(formatCmdError(e, t));
    }
  }

  return (
    <div className="view setup">
      <header className="setup__intro">
        <h1 className="setup__title">{t('setup.title')}</h1>
        <p className="hint">{updateRegrant ? t('setup.updateRegrant') : t('setup.intro')}</p>
      </header>

      <Group
        note={
          error ? (
            <span className="hint--err" role="alert">
              {error}
            </span>
          ) : (
            t('setup.adminNote')
          )
        }
      >
        <PermissionRow
          granted={permissions.accessibility}
          title={t('setup.accessibility')}
          why={t('setup.accessibilityWhy')}
          onRequest={() => void request('accessibility', api.requestAccessibility)}
        />
        <PermissionRow
          granted={permissions.inputMonitoring}
          title={t('setup.inputMonitoring')}
          why={t('setup.inputMonitoringWhy')}
          onRequest={() => void request('inputMonitoring', api.requestInputMonitoring)}
        />
      </Group>

      {allGranted && <p className="hint">{t('setup.allSet')}</p>}

      <div className="setup__footer">
        {allGranted ? (
          <button type="button" className="btn btn--primary" onClick={onDone}>
            {t('setup.done')}
          </button>
        ) : (
          <button type="button" className="btn btn--ghost" onClick={onDismiss}>
            {t('setup.later')}
          </button>
        )}
      </div>
    </div>
  );
}

function PermissionRow({
  granted,
  title,
  why,
  onRequest,
}: {
  granted: boolean;
  title: string;
  why: string;
  onRequest: () => void;
}) {
  const t = useT();
  return (
    <div className="item">
      <div className="item__body">
        <span className="item__title">{title}</span>
        <span className="item__desc">{why}</span>
      </div>
      <div className="item__trail">
        {granted ? (
          <Chip tone="ok">{t('setup.granted')}</Chip>
        ) : (
          <button type="button" className="btn btn--primary" onClick={onRequest}>
            {t('setup.grant')}
          </button>
        )}
      </div>
    </div>
  );
}
