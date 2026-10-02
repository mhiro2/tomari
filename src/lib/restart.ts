import { useRef, useState } from 'react';

import * as api from './api';
import { formatCmdError } from './errors';
import { useT } from './i18n';
import { useSettings } from './settings';

export function useRestartApp() {
  const t = useT();
  const { withSettingsSaved, configurationBusy } = useSettings();
  const inFlight = useRef(false);
  const [restarting, setRestarting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function restart() {
    if (inFlight.current || configurationBusy) return;
    inFlight.current = true;
    setRestarting(true);
    setError(null);
    try {
      await withSettingsSaved(api.restartApp);
      // The acknowledged restart owns the remaining shutdown. Keep the button
      // locked until this process exits instead of submitting another request.
    } catch (failure) {
      setError(t('app.restartFailed', { error: formatCmdError(failure, t) }));
      inFlight.current = false;
      setRestarting(false);
    }
  }

  return { restart, restarting, error, busy: restarting || configurationBusy };
}
