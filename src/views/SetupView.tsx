// The focused permission checklist, opened from the persistent reminder while
// a permission is missing. Permission state lives in App (fed by the initial
// setup_status pull and "tomari:permissions-changed"); this view only renders
// it and forwards grant requests, reporting an immediate grant back up so the
// row flips without waiting for the backend's next poll tick.

import { useEffect, useRef, useState } from 'react';

import { Chip, Group } from '../components/ui';
import * as api from '../lib/api';
import { formatCmdError } from '../lib/errors';
import { acceleratorChips } from '../lib/format';
import { useT } from '../lib/i18n';
import { useSettings } from '../lib/settings';

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
  const { settings } = useSettings();
  const [error, setError] = useState<string | null>(null);
  const [requesting, setRequesting] = useState<keyof SetupPermissions | null>(null);
  const [requested, setRequested] = useState<Partial<Record<keyof SetupPermissions, boolean>>>({});
  const allGranted = permissions.accessibility && permissions.inputMonitoring;
  // The view replaces the shell (often unmounting the very button that opened
  // it), so move focus to the heading — otherwise keyboard and screen-reader
  // users are left focused on nothing.
  const titleRef = useRef<HTMLHeadingElement>(null);
  useEffect(() => {
    titleRef.current?.focus();
  }, []);

  // The "try it" hint shows whatever accelerator is actually bound to the
  // left-half snap right now (the user may have rebound the seeded ⌃⌥←), so
  // it is looked up rather than hardcoded; no binding — or a failed read —
  // just drops the line.
  const [tryItChips, setTryItChips] = useState<string[]>([]);
  useEffect(() => {
    void (async () => {
      try {
        const hotkeys = await api.listHotkeys();
        const snapLeft = hotkeys.find(
          (hk) =>
            hk.enabled &&
            (hk.action.type === 'snapWindow' || hk.action.type === 'snapWindowExact') &&
            hk.action.value === 'leftHalf',
        );
        setTryItChips(acceleratorChips(snapLeft?.accelerator));
      } catch {
        setTryItChips([]);
      }
    })();
  }, []);

  async function request(key: keyof SetupPermissions, call: () => Promise<boolean>) {
    if (requesting !== null) return;
    setRequesting(key);
    setError(null);
    try {
      const ok = await call();
      if (ok) onGranted({ [key]: true });
      else setRequested((current) => ({ ...current, [key]: true }));
    } catch (e) {
      setError(formatCmdError(e, t));
    } finally {
      setRequesting(null);
    }
  }

  return (
    <div className="view setup">
      <header className="setup__intro">
        <h1 className="setup__title" tabIndex={-1} ref={titleRef}>
          {t('setup.title')}
        </h1>
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
          busy={requesting === 'accessibility'}
          disabled={requesting !== null}
          requested={requested.accessibility === true}
          onRequest={() => void request('accessibility', api.requestAccessibility)}
        />
        <PermissionRow
          granted={permissions.inputMonitoring}
          title={t('setup.inputMonitoring')}
          why={t('setup.inputMonitoringWhy')}
          busy={requesting === 'inputMonitoring'}
          disabled={requesting !== null}
          requested={requested.inputMonitoring === true}
          onRequest={() => void request('inputMonitoring', api.requestInputMonitoring)}
        />
      </Group>

      {allGranted && (
        <>
          <p className="hint">{t('setup.allSet')}</p>
          {/* Only promise a snap that can actually happen: the hint needs a
              live binding *and* the window-management master switch on. */}
          {tryItChips.length > 0 && settings?.windowManagementEnabled === true && (
            <TryItHint chips={tryItChips} />
          )}
        </>
      )}

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

// The first-success hint, with the shortcut rendered as native keycap chips.
// `t()` leaves the `{keys}` placeholder in place when no param is given, so
// splitting on it lets the <kbd> elements sit inside the translated sentence.
function TryItHint({ chips }: { chips: string[] }) {
  const t = useT();
  const [before, after] = t('setup.tryIt').split('{keys}');
  return (
    <p className="hint">
      {before}
      <span className="accel">
        {/* A canonical accelerator never repeats a token, so the glyph is a
            stable key (same convention as ShortcutRecorder). */}
        {chips.map((chip) => (
          <kbd key={chip}>{chip}</kbd>
        ))}
      </span>
      {after}
    </p>
  );
}

function PermissionRow({
  granted,
  title,
  why,
  busy,
  disabled,
  requested,
  onRequest,
}: {
  granted: boolean;
  title: string;
  why: string;
  busy: boolean;
  disabled: boolean;
  requested: boolean;
  onRequest: () => void;
}) {
  const t = useT();
  return (
    <div className="item">
      <div className="item__body">
        <span className="item__title">{title}</span>
        <span className="item__desc">{why}</span>
        {!granted && requested && (
          <span className="item__desc permission-row__followup">{t('setup.returnHint')}</span>
        )}
      </div>
      <div className="item__trail">
        {!granted && (
          <button
            type="button"
            className="btn btn--primary"
            // Both rows share the visible System Settings action; the accessible
            // name keeps that label verbatim (so voice control still matches
            // it) and appends the permission so the buttons stay
            // distinguishable out of context.
            aria-label={t('setup.grantFor', { name: title })}
            onClick={onRequest}
            disabled={disabled}
          >
            {t(busy ? 'setup.requesting' : requested ? 'setup.openAgain' : 'setup.grant')}
          </button>
        )}
        {/* The live region (<output> = role "status") is mounted empty from
            the start: only content *added* to an existing region is reliably
            announced, so the "Granted" chip must appear inside it rather than
            arrive with it. */}
        <output>{granted && <Chip tone="ok">{t('setup.granted')}</Chip>}</output>
      </div>
    </div>
  );
}
