import { useEffect, useRef, useState } from 'react';

import { FeaturePageHeader, SegmentedPageNav } from '../components/ui';
import * as displays from '../lib/displays';
import type { DisplayInfo, DisplaySnapshot } from '../lib/displays';
import { useT } from '../lib/i18n';
import { useDisplayState } from '../lib/useDisplayState';
import { DisplayProfiles } from './DisplayProfiles';

type DisplayTab = 'resolution' | 'brightness' | 'arrangement' | 'profiles';

export function DisplayView() {
  const t = useT();
  const [tab, setTab] = useState<DisplayTab>('resolution');
  const { snapshot, selectedId, setSelectedId, error, loadError, busy, loading, refresh, run } =
    useDisplayState();
  const confirmationRef = useRef<HTMLHeadingElement>(null);
  const pendingToken = snapshot?.pending?.token;

  useEffect(() => {
    if (pendingToken === undefined) return;
    confirmationRef.current?.focus();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      void run(() => displays.revertDisplayMode(pendingToken));
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [pendingToken, run]);

  const ambiguousIds = displays.ambiguousDisplayIds(snapshot?.displays ?? []);
  const hasMirroring = Boolean(snapshot?.displays.some((display) => display.mirrored));
  const incompleteModes = Boolean(
    snapshot?.displays.some((display) => display.currentMode === null),
  );
  const configurationUnavailable = ambiguousIds.size > 0 || hasMirroring || incompleteModes;
  const selected =
    snapshot?.displays.find(
      (display) => display.id === selectedId && !ambiguousIds.has(display.id),
    ) ??
    snapshot?.displays.find((display) => !ambiguousIds.has(display.id)) ??
    snapshot?.displays[0];

  return (
    <div className="feature-page display-page">
      <FeaturePageHeader title={t('display.title')} description={t('display.description')} />
      <div className="display-toolbar">
        <h2>{t('display.arrangement')}</h2>
        <button
          type="button"
          className="btn btn--ghost"
          disabled={busy || !snapshot?.displays.length || ambiguousIds.size > 0 || hasMirroring}
          title={t('display.identifyHint')}
          onClick={() => {
            if (snapshot)
              void run(() =>
                displays.identifyDisplays(snapshot.displays.map((display) => display.id)),
              );
          }}
        >
          {t('display.identify')}
        </button>
        {snapshot?.displays.some((display) => display.softwareDimming > 0) && (
          <button
            type="button"
            className="btn btn--ghost"
            disabled={busy}
            onClick={() => {
              void run(displays.resetDisplayDimming);
            }}
          >
            {t('display.resetDimming')}
          </button>
        )}
        <button
          type="button"
          className="btn btn--ghost"
          disabled={busy}
          onClick={() => {
            void refresh();
          }}
        >
          {t('common.refresh')}
        </button>
      </div>
      {loadError && (
        <p className="alert" role="alert">
          {t('display.loadFailed')}
        </p>
      )}
      {ambiguousIds.size > 0 && <output className="alert">{t('display.ambiguousIdentity')}</output>}
      {hasMirroring && <output className="display-notice">{t('display.error.mirrored')}</output>}
      {incompleteModes && (
        <output className="display-notice">{t('display.incompleteConfiguration')}</output>
      )}
      {error && (
        <p className="alert" role="alert">
          {t(error)}
        </p>
      )}
      {loading && <output>{t('display.loading')}</output>}
      {snapshot?.notice && (
        <output className="display-notice">{t(`display.notice.${snapshot.notice}`)}</output>
      )}
      {snapshot?.pending && (
        <section className="display-confirmation" aria-label={t('display.previewTitle')}>
          <div>
            <h2 ref={confirmationRef} tabIndex={-1}>
              {t('display.previewTitle')}
            </h2>
            {snapshot.notice !== 'restoreFailed' && (
              <p>
                {t('display.previewHint', {
                  seconds: Math.ceil(snapshot.pending.remainingMs / 1000),
                })}
              </p>
            )}
          </div>
          <div className="display-actions">
            <button
              type="button"
              className="btn btn--ghost"
              disabled={busy}
              onClick={() => {
                const token = snapshot.pending?.token;
                if (token !== undefined) void run(() => displays.revertDisplayMode(token));
              }}
            >
              {t('display.revert')}
            </button>
            <button
              type="button"
              className="btn btn--primary"
              disabled={busy || configurationUnavailable || snapshot.notice === 'restoreFailed'}
              onClick={() => {
                const token = snapshot.pending?.token;
                if (token !== undefined) void run(() => displays.confirmDisplayMode(token));
              }}
            >
              {t('display.keep')}
            </button>
          </div>
        </section>
      )}
      {snapshot && snapshot.displays.length === 0 && (
        <p className="display-empty">{t('display.empty')}</p>
      )}
      {snapshot && snapshot.displays.length > 0 && (
        <>
          <div className="display-picker" aria-label={t('display.arrangement')}>
            {snapshot.displays.map((display, index) => (
              <button
                type="button"
                key={display.connectionId}
                className={`display-picker__item ${selected === display ? 'is-selected' : ''}`}
                aria-pressed={selected === display}
                disabled={ambiguousIds.has(display.id)}
                onClick={() => setSelectedId(display.id)}
              >
                <strong>
                  {index + 1}. {displays.displayName(display, t)}
                </strong>
                <span>{display.currentMode ? displays.modeLabel(display.currentMode) : '—'}</span>
                {display.main && <small>{t('display.main')}</small>}
              </button>
            ))}
          </div>
          <SegmentedPageNav
            idBase="display-settings"
            label={t('display.controls')}
            value={tab}
            onChange={setTab}
            items={[
              { value: 'resolution', label: t('display.resolution') },
              { value: 'brightness', label: t('display.brightness') },
              { value: 'arrangement', label: t('display.position') },
              { value: 'profiles', label: t('display.profiles') },
            ]}
          />
          <div
            id="display-settings-panel"
            role="tabpanel"
            aria-labelledby={`display-settings-${tab}-tab`}
            className="display-tab-panel"
          >
            {tab === 'profiles' && (
              <DisplayProfiles
                snapshot={snapshot}
                busy={busy}
                configurationUnavailable={configurationUnavailable}
                run={run}
              />
            )}
            {tab === 'arrangement' && (
              <>
                <DisplayMap
                  displays={snapshot.displays}
                  selectedId={selected?.id ?? ''}
                  ambiguousIds={ambiguousIds}
                  onSelect={setSelectedId}
                />

                {selected && snapshot.displays.length > 1 ? (
                  <DisplayArrangement
                    key={selected.id}
                    display={selected}
                    connected={snapshot.displays}
                    disabled={busy || snapshot.pending !== null || configurationUnavailable}
                    run={run}
                  />
                ) : (
                  <p className="display-detail">{t('display.singleDisplay')}</p>
                )}
              </>
            )}
            {selected && tab === 'resolution' && (
              <DisplayResolution
                key={`${selected.id}:${selected.currentMode?.id}`}
                display={selected}
                disabled={busy || snapshot.pending !== null || configurationUnavailable}
                run={run}
              />
            )}
            {selected && tab === 'brightness' && (
              <>
                <DisplayBrightness
                  key={`${selected.id}:${selected.brightness}`}
                  display={selected}
                  disabled={busy || snapshot.pending !== null || ambiguousIds.has(selected.id)}
                  run={run}
                />
                <DisplayDimming
                  key={`${selected.id}:${selected.softwareDimming}`}
                  display={selected}
                  disabled={
                    busy ||
                    snapshot.pending !== null ||
                    selected.mirrored ||
                    ambiguousIds.has(selected.id)
                  }
                  run={run}
                />
              </>
            )}
          </div>
        </>
      )}
      <div className="display-system-settings">
        <button
          type="button"
          className="btn btn--ghost"
          disabled={busy}
          onClick={() => {
            void run(displays.openSystemDisplaySettings);
          }}
        >
          {t('display.systemSettings')}
        </button>
      </div>
    </div>
  );
}

function DisplayMap({
  displays: connected,
  selectedId,
  ambiguousIds,
  onSelect,
}: {
  displays: DisplayInfo[];
  selectedId: string;
  ambiguousIds: Set<string>;
  onSelect: (id: string) => void;
}) {
  const t = useT();
  const minX = Math.min(...connected.map((display) => display.x));
  const minY = Math.min(...connected.map((display) => display.y));
  const width = Math.max(1, ...connected.map((display) => display.x + display.width - minX));
  const height = Math.max(1, ...connected.map((display) => display.y + display.height - minY));
  const scale = Math.min(560 / width, 150 / height);
  return (
    <div className="display-map">
      <div className="display-map__canvas" style={{ width: width * scale, height: height * scale }}>
        {connected.map((display, index) => (
          <button
            type="button"
            key={display.connectionId}
            className={`display-map__screen ${display.id === selectedId && !ambiguousIds.has(display.id) ? 'is-selected' : ''}`}
            style={{
              left: (display.x - minX) * scale,
              top: (display.y - minY) * scale,
              width: Math.max(12, display.width * scale),
              height: Math.max(12, display.height * scale),
            }}
            aria-label={t('display.select', {
              name: `${index + 1}. ${displays.displayName(display, t)}`,
            })}
            aria-pressed={display.id === selectedId && !ambiguousIds.has(display.id)}
            disabled={ambiguousIds.has(display.id)}
            onClick={() => onSelect(display.id)}
          >
            {display.main && <span className="display-map__menubar" />}
            <span>
              {index + 1}. {displays.displayName(display, t)}
            </span>
          </button>
        ))}
      </div>
    </div>
  );
}

type DisplayControlProps = {
  display: DisplayInfo;
  disabled: boolean;
  run: (operation: () => Promise<DisplaySnapshot>) => Promise<void>;
};

function DisplayResolution({ display, disabled, run }: DisplayControlProps) {
  const t = useT();
  const [modeId, setModeId] = useState(display.currentMode?.id ?? display.modes[0]?.id ?? 0);
  const mode = display.modes.find((candidate) => candidate.id === modeId);
  // This is an editable filter, reset by the parent key when the active mode changes.
  // oxlint-disable-next-line react-doctor/no-derived-useState
  const [showLowResolution, setShowLowResolution] = useState(!display.currentMode?.hiDpi);
  const groups = displays.resolutionGroups(display.modes, display.currentMode?.id);
  const selectedGroup = groups.find((group) =>
    group.modes.some((candidate) => candidate.id === modeId),
  );
  const visibleGroups = groups.filter(
    (group) => showLowResolution || group.modes[0]?.hiDpi || group.key === selectedGroup?.key,
  );

  return (
    <section className="display-control">
      <div className="display-control__heading">
        <h2>{t('display.resolution')}</h2>
        <p>{t('display.resolutionHint')}</p>
      </div>
      {!display.mirrored && (
        <>
          <label className="display-field">
            <span>{t('display.resolution')}</span>
            <select
              value={selectedGroup?.key ?? ''}
              disabled={disabled || display.currentMode === null}
              onChange={(event) => {
                const group = groups.find((candidate) => candidate.key === event.target.value);
                const next = group ? displays.preferredMode(group, mode) : undefined;
                if (next) setModeId(next.id);
              }}
            >
              {visibleGroups.map((group) => (
                <option key={group.key} value={group.key}>
                  {group.label}
                </option>
              ))}
            </select>
          </label>
          <label className="display-field display-refresh-rate">
            <span>{t('display.refreshRate')}</span>
            <select
              value={modeId}
              disabled={disabled || !selectedGroup || selectedGroup.modes.length < 2}
              onChange={(event) => setModeId(Number(event.target.value))}
            >
              {selectedGroup?.modes.map((candidate) => (
                <option key={candidate.id} value={candidate.id}>
                  {candidate.refreshRate > 0
                    ? `${Number(candidate.refreshRate.toFixed(2))} Hz`
                    : t('display.automaticRate')}
                </option>
              ))}
            </select>
          </label>
          {display.modes.some((candidate) => candidate.hiDpi) &&
            display.modes.some((candidate) => !candidate.hiDpi) && (
              <label className="display-low-resolution">
                <input
                  type="checkbox"
                  checked={showLowResolution}
                  disabled={disabled}
                  onChange={(event) => setShowLowResolution(event.target.checked)}
                />
                <span>{t('display.showLowResolution')}</span>
              </label>
            )}
          {mode && (
            <p className="display-detail">
              {t('display.pixels', { width: mode.pixelWidth, height: mode.pixelHeight })}
            </p>
          )}
          <button
            type="button"
            className="btn btn--primary"
            disabled={
              disabled || modeId === display.currentMode?.id || !mode || !display.currentMode
            }
            onClick={() => {
              void run(() => displays.previewDisplayMode(display.id, modeId));
            }}
          >
            {t('display.preview')}
          </button>
        </>
      )}
    </section>
  );
}

function DisplayBrightness({ display, disabled, run }: DisplayControlProps) {
  const t = useT();
  const [brightness, setBrightness] = useState(() => Math.round((display.brightness ?? 1) * 100));
  return (
    <section className="display-control">
      <div className="display-control__heading">
        <h2>{t('display.brightness')}</h2>
        <p>
          {display.brightness === null
            ? t('display.brightnessUnavailable')
            : t('display.brightnessHint')}
        </p>
      </div>
      {display.brightness !== null && (
        <>
          <div className="display-brightness">
            <input
              type="range"
              aria-label={t('display.brightness')}
              min="0"
              max="100"
              step="1"
              value={brightness}
              disabled={disabled}
              onChange={(event) => setBrightness(Number(event.target.value))}
            />
            <output>{brightness}%</output>
          </div>
          <button
            type="button"
            className="btn btn--ghost"
            disabled={disabled || brightness === Math.round(display.brightness * 100)}
            onClick={() => {
              void run(() => displays.setDisplayBrightness(display.id, brightness / 100));
            }}
          >
            {t('display.applyBrightness')}
          </button>
        </>
      )}
    </section>
  );
}

function DisplayArrangement({
  display,
  connected,
  disabled,
  run,
}: {
  display: DisplayInfo;
  connected: DisplayInfo[];
  disabled: boolean;
  run: (operation: () => Promise<DisplaySnapshot>) => Promise<void>;
}) {
  const t = useT();
  const others = connected.filter((item) => item.id !== display.id);
  const [referenceId, setReferenceId] = useState(others[0]?.id ?? '');
  const [placement, setPlacement] = useState<displays.RelativePlacement>('right');
  const reference = others.find((item) => item.id === referenceId) ?? others[0];
  return (
    <section className="display-control display-arrangement">
      <div className="display-control__heading">
        <h2>{t('display.position')}</h2>
        <p>{t('display.positionHint')}</p>
      </div>
      <div className="display-arrangement__fields">
        <label className="display-field">
          <span>{t('display.relativeTo')}</span>
          <select
            value={reference?.id ?? ''}
            disabled={disabled}
            onChange={(event) => setReferenceId(event.target.value)}
          >
            {others.map((item) => (
              <option key={item.connectionId} value={item.id}>
                {connected.indexOf(item) + 1}. {displays.displayName(item, t)}
              </option>
            ))}
          </select>
        </label>
        <label className="display-field">
          <span>{t('display.placement')}</span>
          <select
            value={placement}
            disabled={disabled}
            onChange={(event) => setPlacement(event.target.value as displays.RelativePlacement)}
          >
            {(['left', 'right', 'above', 'below'] as const).map((position) => (
              <option key={position} value={position}>
                {t(`display.${position}`)}
              </option>
            ))}
          </select>
        </label>
      </div>
      <div className="display-actions">
        <button
          type="button"
          className="btn btn--ghost"
          disabled={disabled || !reference}
          onClick={() => {
            if (reference)
              void run(() => displays.previewDisplayPosition(display.id, reference.id, placement));
          }}
        >
          {t('display.tryPosition')}
        </button>
        {!display.main && (
          <button
            type="button"
            className="btn btn--ghost"
            disabled={disabled}
            onClick={() => {
              void run(() => displays.previewMainDisplay(display.id));
            }}
          >
            {t('display.makeMain')}
          </button>
        )}
      </div>
    </section>
  );
}

function DisplayDimming({
  display,
  disabled,
  run,
}: {
  display: DisplayInfo;
  disabled: boolean;
  run: (operation: () => Promise<DisplaySnapshot>) => Promise<void>;
}) {
  const t = useT();
  const [amount, setAmount] = useState(() => Math.round(display.softwareDimming * 100));
  return (
    <section className="display-control display-dimming">
      <div className="display-control__heading">
        <h2>{t('display.dimming')}</h2>
        <p>{t('display.dimmingHint')}</p>
      </div>
      <div className="display-brightness">
        <input
          type="range"
          aria-label={t('display.dimming')}
          min="0"
          max="80"
          step="1"
          value={amount}
          disabled={disabled}
          onChange={(event) => setAmount(Number(event.target.value))}
        />
        <output>{amount}%</output>
      </div>
      <p className="display-detail">{t('display.dimmingSession')}</p>
      <button
        type="button"
        className="btn btn--ghost"
        disabled={disabled || amount === Math.round(display.softwareDimming * 100)}
        onClick={() => {
          void run(() => displays.setDisplayDimming(display.id, amount / 100));
        }}
      >
        {t('display.applyDimming')}
      </button>
    </section>
  );
}
