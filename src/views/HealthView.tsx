import { useCallback, useEffect, useRef, useState } from 'react';

import { FeaturePageHeader, SettingsList, SettingsRow } from '../components/ui';
import * as api from '../lib/api';
import { formatCmdError } from '../lib/errors';
import { useT, type Translator } from '../lib/i18n';
import type { DiagnosticsSnapshot, SupportBundleExport } from '../lib/types';

type HealthTone = 'ready' | 'attention' | 'pending' | 'inactive';

export type HealthDestination =
  | { section: 'keyboard'; tab: 'modifiers' | 'shortcuts' }
  | { section: 'window'; tab: 'mouse' }
  | { section: 'menubar'; tab: 'items' }
  | { section: 'session' };

type HealthAction = {
  label: string;
  onClick: () => void;
};

// One row of the report. The tone is the only state encoding on the page: a
// dot in the row's lead, and the same dot in the summary. The description
// says in words what the dot says in color, so nothing depends on color alone.
type HealthItem = {
  id: string;
  title: string;
  tone: HealthTone;
  description: string;
  counters?: string;
  action?: HealthAction;
};

type Navigation = {
  onNavigate?: (destination: HealthDestination) => void;
  onOpenPermissions?: () => void;
};

type Tap = DiagnosticsSnapshot['taps'][number];

export function HealthView({ onNavigate, onOpenPermissions }: Navigation = {}) {
  const t = useT();
  const [snapshot, setSnapshot] = useState<DiagnosticsSnapshot | null>(null);
  const [loadError, setLoadError] = useState<unknown>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [exported, setExported] = useState<SupportBundleExport | null>(null);
  const [exportError, setExportError] = useState<unknown>(null);
  const requestGeneration = useRef(0);

  const refresh = useCallback(async () => {
    const generation = ++requestGeneration.current;
    setRefreshing(true);
    setLoadError(null);
    try {
      const next = await api.getDiagnostics();
      if (requestGeneration.current === generation) setSnapshot(next);
    } catch (error) {
      if (requestGeneration.current === generation) setLoadError(error);
    } finally {
      // A stale Strict Mode request must not overwrite the busy state chosen
      // by the newest request when the two complete out of order.
      setRefreshing((current) => (requestGeneration.current === generation ? false : current));
    }
  }, []);

  useEffect(() => {
    void refresh();
    return () => {
      requestGeneration.current += 1;
    };
  }, [refresh]);

  async function exportBundle() {
    if (exporting) return;
    setExporting(true);
    setExportError(null);
    setExported(null);
    try {
      setExported(await api.exportSupportBundle());
    } catch (error) {
      setExportError(error);
    } finally {
      setExporting(false);
    }
  }

  const navigation: Navigation = { onNavigate, onOpenPermissions };
  const inputItems = snapshot ? inputPathItems(snapshot, t, navigation) : [];
  const systemItems = snapshot ? systemTrustItems(snapshot, t, navigation) : [];
  const items = [...inputItems, ...systemItems];
  const attentionCount = items.filter(({ tone }) => tone === 'attention').length;
  const pendingCount = items.filter(({ tone }) => tone === 'pending').length;
  const summaryTone: HealthTone =
    attentionCount > 0 ? 'attention' : pendingCount > 0 ? 'pending' : 'ready';

  return (
    <div className="view health-view" aria-busy={refreshing}>
      <FeaturePageHeader title={t('health.title')} description={t('health.pageDescription')} />

      {snapshot === null ? (
        <section className="health-loading" aria-live="polite">
          {loadError === null ? (
            <>
              <span className="loading-mark" aria-hidden="true" />
              <span>{t('health.reading')}</span>
            </>
          ) : (
            <>
              <span role="alert">
                {t('health.loadFailed', { error: formatCmdError(loadError, t) })}
              </span>
              <button type="button" className="btn" onClick={() => void refresh()}>
                {t('common.retry')}
              </button>
            </>
          )}
        </section>
      ) : (
        <>
          <section
            className={`health-summary health-summary--${summaryTone}`}
            aria-labelledby="health-summary-title"
          >
            <HealthDot tone={summaryTone} />
            <div className="health-summary__copy">
              <h2 id="health-summary-title">
                {attentionCount > 0
                  ? t(attentionCount === 1 ? 'health.needsAttentionOne' : 'health.needsAttention', {
                      count: attentionCount,
                    })
                  : pendingCount > 0
                    ? t(pendingCount === 1 ? 'health.pendingSummaryOne' : 'health.pendingSummary', {
                        count: pendingCount,
                      })
                    : t('health.allReady')}
              </h2>
              <p>
                {t('health.snapshotMeta', {
                  version: snapshot.app.version,
                  architecture: snapshot.app.architecture,
                })}
              </p>
              {loadError !== null && (
                <p className="health-summary__error" role="alert">
                  {t('health.refreshFailed', { error: formatCmdError(loadError, t) })}
                </p>
              )}
            </div>
            <button
              type="button"
              className="btn btn--ghost"
              disabled={refreshing}
              onClick={() => void refresh()}
            >
              {refreshing ? t('health.refreshing') : t('common.refresh')}
            </button>
          </section>

          <SettingsList label={t('health.group.input')}>
            {inputItems.map((item) => (
              <HealthRow key={item.id} item={item} />
            ))}
          </SettingsList>

          <SettingsList label={t('health.group.system')}>
            {systemItems.map((item) => (
              <HealthRow key={item.id} item={item} />
            ))}
          </SettingsList>

          <SettingsList label={t('health.group.support')}>
            <SettingsRow
              title={t('health.supportBundle')}
              description={t('health.supportBundleDescription')}
              trail={
                <button
                  type="button"
                  className="btn"
                  disabled={exporting}
                  onClick={() => void exportBundle()}
                >
                  {exporting ? t('health.exporting') : t('health.export')}
                </button>
              }
            />
            {exported !== null && (
              <output className="health-note" aria-live="polite">
                <span>{t('health.exported')}</span>
                <span className="health-note__path">{exported.path}</span>
              </output>
            )}
            {exportError !== null && (
              <p className="health-note health-note--error" role="alert">
                {t('health.exportFailed', { error: formatCmdError(exportError, t) })}
              </p>
            )}
          </SettingsList>
        </>
      )}
    </div>
  );
}

// The dot is decorative; the state name travels with it for assistive
// technology so a row never depends on color alone.
function HealthDot({ tone }: { tone: HealthTone }) {
  const t = useT();
  return (
    <span className="health-dot-lead">
      <span className={`health-dot health-dot--${tone}`} aria-hidden="true" />
      <span className="sr-only">{t(`health.tone.${tone}`)}</span>
    </span>
  );
}

function HealthRow({ item }: { item: HealthItem }) {
  return (
    <SettingsRow
      className="health-row"
      lead={<HealthDot tone={item.tone} />}
      title={item.title}
      description={
        <>
          {item.description}
          {item.counters !== undefined && <span className="health-counters">{item.counters}</span>}
        </>
      }
      trail={
        item.action && (
          <button type="button" className="btn" onClick={item.action.onClick}>
            {item.action.label}
          </button>
        )
      }
    />
  );
}

function inputPathItems(
  snapshot: DiagnosticsSnapshot,
  t: Translator,
  navigation: Navigation,
): HealthItem[] {
  return [
    permissionsItem(snapshot, t, navigation),
    ...snapshot.taps.map((tap) => tapItem(tap, t, navigation)),
    capsLockItem(snapshot, t, navigation),
    shortcutsItem(snapshot, t, navigation),
  ];
}

function systemTrustItems(
  snapshot: DiagnosticsSnapshot,
  t: Translator,
  navigation: Navigation,
): HealthItem[] {
  return [
    menuBarItem(snapshot, t, navigation),
    keepAwakeItem(snapshot, t, navigation),
    databaseItem(snapshot, t),
    updaterItem(snapshot, t),
  ];
}

function permissionsItem(
  snapshot: DiagnosticsSnapshot,
  t: Translator,
  { onOpenPermissions }: Navigation,
): HealthItem {
  const { accessibility, inputMonitoring } = snapshot.permissions;
  const title = t('health.permissions');
  if (accessibility && inputMonitoring) {
    return { id: 'permissions', title, tone: 'ready', description: t('health.permissionsGranted') };
  }
  return {
    id: 'permissions',
    title,
    tone: 'attention',
    description: t(
      !accessibility && !inputMonitoring
        ? 'health.permissionsMissingBoth'
        : accessibility
          ? 'health.permissionsMissingInputMonitoring'
          : 'health.permissionsMissingAccessibility',
    ),
    action: makeAction(t('health.action.permissions'), onOpenPermissions),
  };
}

function tapItem(
  tap: Tap,
  t: Translator,
  { onNavigate, onOpenPermissions }: Navigation,
): HealthItem {
  const id = `tap-${tap.kind}`;
  const title = t(`health.tap.${tap.kind}`);
  const total = tap.restartCount + tap.disableCount + tap.recoveryCount;
  const counters =
    total > 0
      ? t('health.tapCounters', {
          restarts: tap.restartCount,
          disables: tap.disableCount,
          recoveries: tap.recoveryCount,
        })
      : undefined;

  if (!tap.enabled) {
    return { id, title, tone: 'inactive', description: t('health.tapOff'), counters };
  }
  if (tap.state === 'healthy') {
    return { id, title, tone: 'ready', description: t('health.tapRunning'), counters };
  }
  if (tap.state === 'starting' || tap.state === 'disabledByTimeout') {
    return { id, title, tone: 'pending', description: t(`health.tapState.${tap.state}`), counters };
  }
  const action =
    tap.state === 'permissionDenied'
      ? makeAction(t('health.action.permissions'), onOpenPermissions)
      : tap.kind === 'keyboard'
        ? destinationAction(
            t('health.action.modifiers'),
            { section: 'keyboard', tab: 'modifiers' },
            onNavigate,
          )
        : destinationAction(
            t('health.action.windowMouse'),
            { section: 'window', tab: 'mouse' },
            onNavigate,
          );
  return {
    id,
    title,
    tone: 'attention',
    description: t(`health.tapState.${tap.state}`),
    counters,
    action,
  };
}

function capsLockItem(
  snapshot: DiagnosticsSnapshot,
  t: Translator,
  { onNavigate }: Navigation,
): HealthItem {
  const { ownership, mappingActive, reconciled } = snapshot.capsLock;
  const id = 'capsLock';
  const title = t('health.capsLock');
  if (!reconciled || ownership === 'pending') {
    return { id, title, tone: 'pending', description: t('health.capsPending') };
  }
  if (ownership === 'unknown' || mappingActive !== (ownership === 'held')) {
    return {
      id,
      title,
      tone: 'attention',
      description: t(ownership === 'unknown' ? 'health.capsUnknown' : 'health.capsMismatch'),
      action: destinationAction(
        t('health.action.modifiers'),
        { section: 'keyboard', tab: 'modifiers' },
        onNavigate,
      ),
    };
  }
  return mappingActive
    ? { id, title, tone: 'ready', description: t('health.capsHeld') }
    : { id, title, tone: 'inactive', description: t('health.capsOff') };
}

function shortcutsItem(
  snapshot: DiagnosticsSnapshot,
  t: Translator,
  { onNavigate }: Navigation,
): HealthItem {
  const { enabled, registrationIncomplete, registeredCount, invalidCount } = snapshot.shortcuts;
  const id = 'shortcuts';
  const title = t('health.shortcuts');
  if (!enabled) {
    return { id, title, tone: 'inactive', description: t('health.shortcutsOff') };
  }
  if (invalidCount > 0 || registrationIncomplete) {
    return {
      id,
      title,
      tone: 'attention',
      description:
        invalidCount > 0
          ? t('health.shortcutsInvalid', { registered: registeredCount, invalid: invalidCount })
          : t('health.shortcutsConflict', { registered: registeredCount }),
      action: destinationAction(
        t('health.action.shortcuts'),
        { section: 'keyboard', tab: 'shortcuts' },
        onNavigate,
      ),
    };
  }
  return {
    id,
    title,
    tone: 'ready',
    description:
      registeredCount === 1
        ? t('health.shortcutsRegisteredOne')
        : t('health.shortcutsRegistered', { registered: registeredCount }),
  };
}

function menuBarItem(
  snapshot: DiagnosticsSnapshot,
  t: Translator,
  { onNavigate, onOpenPermissions }: Navigation,
): HealthItem {
  const { enabled, supported, permissionGranted, dividerAvailable } = snapshot.menuBar;
  const id = 'menuBar';
  const title = t('health.menuBar');
  if (!enabled) {
    return { id, title, tone: 'inactive', description: t('health.menuBarOff') };
  }
  if (!supported) {
    return { id, title, tone: 'inactive', description: t('health.menuBarUnsupported') };
  }
  if (!permissionGranted) {
    return {
      id,
      title,
      tone: 'attention',
      description: t('health.menuBarPermissionMissing'),
      action: makeAction(t('health.action.permissions'), onOpenPermissions),
    };
  }
  if (!dividerAvailable) {
    return {
      id,
      title,
      tone: 'attention',
      description: t('health.menuBarDividerMissing'),
      action: destinationAction(
        t('health.action.menuBar'),
        { section: 'menubar', tab: 'items' },
        onNavigate,
      ),
    };
  }
  return { id, title, tone: 'ready', description: t('health.menuBarReady') };
}

function keepAwakeItem(
  snapshot: DiagnosticsSnapshot,
  t: Translator,
  { onNavigate }: Navigation,
): HealthItem {
  const { active, phase, markerPresent, kernelSleepDisabled, ownsLidClose } = snapshot.keepAwake;
  const id = 'keepAwake';
  const title = t('health.keepAwake');
  const action = destinationAction(
    t('health.action.keepAwake'),
    { section: 'session' },
    onNavigate,
  );

  if (phase === 'enabling' || phase === 'disabling') {
    return { id, title, tone: 'pending', description: t(`health.keepAwake.${phase}`) };
  }
  if (phase === 'failed') {
    return { id, title, tone: 'attention', description: t('health.keepAwake.failed'), action };
  }
  if (!active) {
    // A foreign kernel override is not Tomari's to report; only Tomari's own
    // leftovers (its marker or lid-close ownership) are.
    return markerPresent || ownsLidClose
      ? { id, title, tone: 'attention', description: t('health.keepAwake.residual'), action }
      : { id, title, tone: 'inactive', description: t('health.keepAwake.off') };
  }
  const ownershipCoherent = markerPresent === ownsLidClose;
  return phase === 'on' && ownershipCoherent && kernelSleepDisabled === true
    ? { id, title, tone: 'ready', description: t('health.keepAwake.on') }
    : { id, title, tone: 'attention', description: t('health.keepAwake.mismatch'), action };
}

function databaseItem(snapshot: DiagnosticsSnapshot, t: Translator): HealthItem {
  const database = snapshot.database;
  const id = 'database';
  const title = t('health.database');
  if (database === null) {
    return { id, title, tone: 'attention', description: t('health.databaseUnavailable') };
  }
  if (!database.integrityOk) {
    return { id, title, tone: 'attention', description: t('health.databaseFailed') };
  }
  if (database.schemaVersion !== database.latestSchemaVersion) {
    return {
      id,
      title,
      tone: 'attention',
      description: t('health.databaseOutdated'),
    };
  }
  return { id, title, tone: 'ready', description: t('health.databaseReady') };
}

function updaterItem(snapshot: DiagnosticsSnapshot, t: Translator): HealthItem {
  return snapshot.updater.signatureConfigured
    ? {
        id: 'updater',
        title: t('health.updates'),
        tone: 'ready',
        description: t('health.updateSignatureReady'),
      }
    : {
        id: 'updater',
        title: t('health.updates'),
        tone: 'attention',
        description: t('health.updateSignatureMissing'),
      };
}

function makeAction(label: string, onClick: (() => void) | undefined): HealthAction | undefined {
  return onClick === undefined ? undefined : { label, onClick };
}

function destinationAction(
  label: string,
  destination: HealthDestination,
  onNavigate: ((destination: HealthDestination) => void) | undefined,
): HealthAction | undefined {
  return makeAction(label, onNavigate && (() => onNavigate(destination)));
}
