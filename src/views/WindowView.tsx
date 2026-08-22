import { listen } from '@tauri-apps/api/event';
import { useEffect, useEffectEvent, useLayoutEffect, useRef, useState } from 'react';

import { AddHotkeyForm, HotkeyRow, type HotkeyActionOption } from '../components/HotkeyEditor';
import { Banner, Group, MasterSwitchHeader, SwitchRow } from '../components/ui';
import * as api from '../lib/api';
import { formatCmdError } from '../lib/errors';
import { presetLabel } from '../lib/format';
import { useT, type Translator } from '../lib/i18n';
import { useSettings } from '../lib/settings';
import type {
  AppAction,
  HistoryActionResult,
  Hotkey,
  NormalizedRect,
  PermissionsChanged,
  PlacementContext,
  PlacementSlot,
  WindowHistoryStatus,
  WindowPlacement,
  WindowPreset,
  WindowTarget,
} from '../lib/types';

const STATUS_CLEAR_MS = 4000;
const RECOVERABLE_STATUS_CLEAR_MS = 8000;
const EMPTY_HISTORY: WindowHistoryStatus = { canUndo: false, canRedo: false };
const WINDOW_PRESETS: WindowPreset[] = [
  'leftHalf',
  'rightHalf',
  'topHalf',
  'bottomHalf',
  'topLeftQuarter',
  'topRightQuarter',
  'bottomLeftQuarter',
  'bottomRightQuarter',
  'leftThird',
  'centerThird',
  'rightThird',
  'leftTwoThirds',
  'rightTwoThirds',
  'center',
  'maximize',
];

type Status = { message: string; isError: boolean; undoPlacement: boolean };

function sameTarget(a: WindowTarget | undefined, b: WindowTarget | undefined): boolean {
  return a?.bundleId === b?.bundleId && a?.windowId === b?.windowId;
}

function isWindowTargetChanged(error: unknown): boolean {
  return Boolean(
    error && typeof error === 'object' && 'code' in error && error.code === 'windowTargetChanged',
  );
}

function isWindowAction(action: AppAction): boolean {
  return [
    'snapWindow',
    'snapWindowExact',
    'moveWindowToDisplay',
    'recallWindowPlacement',
    'moveWindowToDisplayAndRecall',
    'undoWindow',
    'redoWindow',
  ].includes(action.type);
}

function shortcutOptions(t: Translator): HotkeyActionOption[] {
  return [
    {
      key: 'recall',
      label: t('action.recallPlacement'),
      action: { type: 'recallWindowPlacement' },
    },
    {
      key: 'moveRecallNext',
      label: t('action.moveAndRecall', { display: t('direction.next') }),
      action: { type: 'moveWindowToDisplayAndRecall', value: 'next' },
    },
    {
      key: 'moveRecallPrev',
      label: t('action.moveAndRecall', { display: t('direction.prev') }),
      action: { type: 'moveWindowToDisplayAndRecall', value: 'prev' },
    },
    {
      key: 'moveNext',
      label: t('action.moveToDisplay', { display: t('direction.next') }),
      action: { type: 'moveWindowToDisplay', value: 'next' },
    },
    {
      key: 'movePrev',
      label: t('action.moveToDisplay', { display: t('direction.prev') }),
      action: { type: 'moveWindowToDisplay', value: 'prev' },
    },
    { key: 'undo', label: t('action.undoWindow'), action: { type: 'undoWindow' } },
    { key: 'redo', label: t('action.redoWindow'), action: { type: 'redoWindow' } },
    ...WINDOW_PRESETS.map(
      (preset): HotkeyActionOption => ({
        key: preset,
        label: t('action.snap', { target: presetLabel(preset, t) }),
        action: { type: 'snapWindow', value: preset },
      }),
    ),
  ];
}

function clearSaving(
  setIds: (fn: (ids: ReadonlySet<string>) => ReadonlySet<string>) => void,
  id: string,
) {
  setIds((ids) => {
    const next = new Set(ids);
    next.delete(id);
    return next;
  });
}

export function WindowView({
  onOpenSetup,
  onOpenKeyboard,
}: {
  onOpenSetup?: () => void;
  onOpenKeyboard?: () => void;
}) {
  const t = useT();
  const { settings, update } = useSettings();
  const [context, setContext] = useState<PlacementContext | null>(null);
  const [history, setHistory] = useState<WindowHistoryStatus>(EMPTY_HISTORY);
  const [activeSlot, setActiveSlot] = useState<PlacementSlot | null>(null);
  const [hotkeys, setHotkeys] = useState<Hotkey[]>([]);
  const [granted, setGranted] = useState(true);
  const [status, setStatus] = useState<Status | null>(null);
  const [busy, setBusy] = useState(false);
  const [savingHotkeyIds, setSavingHotkeyIds] = useState<ReadonlySet<string>>(new Set());
  const clearTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const contextRequestRef = useRef<Promise<PlacementContext> | null>(null);
  const hotkeysRef = useRef(hotkeys);

  useLayoutEffect(() => {
    hotkeysRef.current = hotkeys;
  }, [hotkeys]);

  function showStatus(message: string, isError: boolean, undoPlacement = false) {
    if (clearTimerRef.current !== null) clearTimeout(clearTimerRef.current);
    setStatus({ message, isError, undoPlacement });
    clearTimerRef.current = isError
      ? null
      : setTimeout(
          () => {
            setStatus(null);
            clearTimerRef.current = null;
          },
          undoPlacement ? RECOVERABLE_STATUS_CLEAR_MS : STATUS_CLEAR_MS,
        );
  }

  const reportLoadError = useEffectEvent((error: unknown) =>
    showStatus(formatCmdError(error, t), true),
  );

  async function refreshContext(resetActive: boolean, reportError = true) {
    const request = contextRequestRef.current ?? api.getPlacementContext();
    contextRequestRef.current = request;
    try {
      const next = await request;
      setContext(next);
      if (resetActive) setActiveSlot(null);
      return next;
    } catch (error) {
      setContext(null);
      setActiveSlot(null);
      if (reportError) reportLoadError(error);
      return null;
    } finally {
      if (contextRequestRef.current === request) contextRequestRef.current = null;
    }
  }

  async function refreshHistory(reportError = true) {
    try {
      const next = await api.getWindowHistoryStatus();
      const available = next ?? EMPTY_HISTORY;
      setHistory(available);
      return available;
    } catch (error) {
      setHistory(EMPTY_HISTORY);
      if (reportError) reportLoadError(error);
      return null;
    }
  }

  async function refreshWorkflow(resetActive: boolean, reportError = true) {
    const [nextContext] = await Promise.all([
      refreshContext(resetActive, reportError),
      refreshHistory(reportError),
    ]);
    return nextContext;
  }

  const refreshForPanelFocus = useEffectEvent(() => {
    void refreshWorkflow(true);
  });

  useEffect(() => {
    void api.accessibilityStatus().then(setGranted).catch(reportLoadError);
    refreshForPanelFocus();
    void api
      .listHotkeys()
      .then((items) => setHotkeys(items.filter((item) => isWindowAction(item.action))))
      .catch(reportLoadError);

    const permissionsUnlisten = listen<PermissionsChanged>(
      'tomari:permissions-changed',
      (event) => {
        setGranted(event.payload.accessibility);
        if (event.payload.accessibility) refreshForPanelFocus();
      },
    );
    const panelUnlisten = listen('tomari:panel-shown', refreshForPanelFocus);
    const onFocus = () => refreshForPanelFocus();
    window.addEventListener('focus', onFocus);
    return () => {
      if (clearTimerRef.current !== null) clearTimeout(clearTimerRef.current);
      window.removeEventListener('focus', onFocus);
      void permissionsUnlisten.then((fn) => fn());
      void panelUnlisten.then((fn) => fn());
    };
  }, []);

  async function runTargetAction<T>(action: (target: WindowTarget) => Promise<T>) {
    if (busy || !context) return null;
    const snapshot = context;
    setBusy(true);
    try {
      const result = await action(snapshot.target);
      const nextContext = await refreshWorkflow(false, false);
      if (!sameTarget(snapshot.target, nextContext?.target)) setActiveSlot(null);
      return { result, snapshot, nextContext };
    } catch (error) {
      showStatus(formatCmdError(error, t), true);
      if (isWindowTargetChanged(error)) await refreshWorkflow(true, false);
      return null;
    } finally {
      setBusy(false);
    }
  }

  async function recall() {
    const outcome = await runTargetAction(api.recallWindowPlacement);
    if (!outcome) return;
    const slot = outcome.result;
    if (sameTarget(outcome.snapshot.target, outcome.nextContext?.target)) setActiveSlot(slot);
    showStatus(
      t('window.restoredSlot', {
        app: outcome.snapshot.application.name,
        slot: t(`window.slot.${slot}`),
      }),
      false,
    );
  }

  async function moveAndRecall() {
    const outcome = await runTargetAction((target) =>
      api.moveWindowToDisplayAndRecall(target, 'next'),
    );
    if (!outcome) return;
    if (outcome.result.status === 'noAdjacentDisplay') {
      showStatus(t('window.noAdjacentDisplay'), false);
      return;
    }
    const slot = outcome.result.slot;
    if (sameTarget(outcome.snapshot.target, outcome.nextContext?.target)) setActiveSlot(slot);
    showStatus(
      t('window.movedAndRestoredSlot', {
        app: outcome.snapshot.application.name,
        slot: t(`window.slot.${slot}`),
      }),
      false,
    );
  }

  async function capture(slot: PlacementSlot) {
    const outcome = await runTargetAction((target) => api.captureWindowPlacement(target, slot));
    if (!outcome) return;
    showStatus(
      outcome.result.changed
        ? t('window.remembered', { slot: t(`window.slot.${slot}`) })
        : t('window.alreadyRemembered', { slot: t(`window.slot.${slot}`) }),
      false,
      outcome.result.changed,
    );
  }

  async function forget(slot: PlacementSlot) {
    const outcome = await runTargetAction((target) => api.forgetWindowPlacement(target, slot));
    if (!outcome) return;
    showStatus(
      t('window.forgotten', { slot: t(`window.slot.${slot}`) }),
      false,
      outcome.result.changed,
    );
  }

  async function undoPlacementEdit() {
    if (busy) return;
    setBusy(true);
    try {
      const result = await api.undoWindowPlacementEdit();
      await refreshContext(false, false);
      showStatus(
        result === 'applied' ? t('window.savedEditUndone') : t('window.noSavedEditToUndo'),
        false,
      );
    } catch (error) {
      showStatus(formatCmdError(error, t), true);
    } finally {
      setBusy(false);
    }
  }

  async function changeHistory(action: () => Promise<HistoryActionResult>, verb: 'undo' | 'redo') {
    if (busy) return;
    setBusy(true);
    try {
      const result = await action();
      await refreshWorkflow(false, false);
      const message =
        result === 'applied'
          ? t(verb === 'undo' ? 'window.undone' : 'window.redone')
          : result === 'staleEntriesDiscarded'
            ? t('window.staleHistoryDiscarded')
            : t('window.historyEmpty');
      showStatus(message, false);
    } catch (error) {
      showStatus(formatCmdError(error, t), true);
    } finally {
      setBusy(false);
    }
  }

  async function saveHotkeyPatch(id: string, patch: Partial<Hotkey>) {
    if (savingHotkeyIds.has(id)) return;
    setSavingHotkeyIds((ids) => new Set(ids).add(id));
    const current = hotkeysRef.current.find((hotkey) => hotkey.id === id);
    if (!current) {
      clearSaving(setSavingHotkeyIds, id);
      return;
    }
    const next = { ...current, ...patch };
    try {
      await api.saveHotkey(next);
      setHotkeys((items) => items.map((item) => (item.id === id ? next : item)));
      setStatus(null);
    } catch (error) {
      showStatus(formatCmdError(error, t), true);
    } finally {
      clearSaving(setSavingHotkeyIds, id);
    }
  }

  async function removeHotkey(id: string) {
    try {
      await api.deleteHotkey(id);
      setHotkeys((items) => items.filter((item) => item.id !== id));
    } catch (error) {
      showStatus(formatCmdError(error, t), true);
    }
  }

  async function grant() {
    try {
      const ok = await api.requestAccessibility();
      setGranted(ok);
      if (ok) await refreshWorkflow(true);
    } catch (error) {
      showStatus(formatCmdError(error, t), true);
    }
  }

  if (!settings) return <div className="view">{t('common.loading')}</div>;

  const on = settings.windowManagementEnabled;

  return (
    <div className="view">
      <MasterSwitchHeader
        title={t('settings.windowManagement')}
        checked={on}
        onChange={(value) => update({ windowManagementEnabled: value })}
        offNote={t('window.offNote')}
        enableLabel={t('common.turnOn')}
        toggleLabel={t('common.enable', { label: t('settings.windowManagement') })}
      />

      <div className={`view ${on ? '' : 'gated'}`} aria-disabled={!on} inert={!on}>
        {!granted && (
          <Banner tone="warn">
            <div className="banner__body">
              <strong>{t('window.axNeeded')}</strong>
              <p>{t('window.axBody')}</p>
            </div>
            {onOpenSetup && (
              <button type="button" className="btn btn--ghost" onClick={onOpenSetup}>
                {t('setup.openSetup')}
              </button>
            )}
            <button type="button" className="btn btn--primary" onClick={() => void grant()}>
              {t('window.grantAccess')}
            </button>
          </Banner>
        )}

        <PlacementStage
          context={context}
          history={history}
          activeSlot={activeSlot}
          busy={busy}
          onRefresh={() => void refreshWorkflow(true)}
          onRecall={() => void recall()}
          onMove={() => void moveAndRecall()}
          onUndo={() => void changeHistory(api.undoWindow, 'undo')}
          onRedo={() => void changeHistory(api.redoWindow, 'redo')}
        />

        <MouseControls
          dragToSnapEnabled={settings.dragToSnapEnabled}
          dragToMoveEnabled={settings.dragToMoveEnabled}
          onDragToSnap={(value) => update({ dragToSnapEnabled: value })}
          onDragToMove={(value) => update({ dragToMoveEnabled: value })}
        />

        <RememberedPositions
          context={context}
          activeSlot={activeSlot}
          busy={busy}
          onCapture={(slot) => void capture(slot)}
          onForget={(slot) => void forget(slot)}
        />

        <WindowControls
          keyboardEnabled={settings.keyboardEnabled}
          onOpenKeyboard={onOpenKeyboard}
          hotkeys={hotkeys}
          savingHotkeyIds={savingHotkeyIds}
          onSave={(id, patch) => void saveHotkeyPatch(id, patch)}
          onRemove={(id) => void removeHotkey(id)}
          onAdded={(hotkey) => setHotkeys((items) => [...items, hotkey])}
          onError={(message) => showStatus(message, true)}
        />
      </div>

      {status && (
        <div
          className={`window-toast ${status.isError ? 'window-toast--err' : ''}`}
          role={status.isError ? 'alert' : 'status'}
        >
          <span>{status.message}</span>
          {status.undoPlacement && (
            <button
              type="button"
              className="btn btn--ghost"
              onClick={() => void undoPlacementEdit()}
              disabled={busy}
            >
              {t('common.undo')}
            </button>
          )}
        </div>
      )}
    </div>
  );
}

function RememberedPositions({
  context,
  activeSlot,
  busy,
  onCapture,
  onForget,
}: {
  context: PlacementContext | null;
  activeSlot: PlacementSlot | null;
  busy: boolean;
  onCapture: (slot: PlacementSlot) => void;
  onForget: (slot: PlacementSlot) => void;
}) {
  const t = useT();
  return (
    <Group label={t('window.rememberedHomes')} note={t('window.rememberedHomesHint')}>
      <div className="placement-slots">
        {(['primary', 'secondary'] as const).map((slot) => (
          <PlacementSlotCard
            key={slot}
            slot={slot}
            placement={context?.placements.find((placement) => placement.slot === slot)}
            active={activeSlot === slot}
            busy={busy || !context}
            onCapture={() => onCapture(slot)}
            onForget={() => onForget(slot)}
          />
        ))}
      </div>
    </Group>
  );
}

function WindowControls({
  keyboardEnabled,
  onOpenKeyboard,
  hotkeys,
  savingHotkeyIds,
  onSave,
  onRemove,
  onAdded,
  onError,
}: {
  keyboardEnabled: boolean;
  onOpenKeyboard?: () => void;
  hotkeys: Hotkey[];
  savingHotkeyIds: ReadonlySet<string>;
  onSave: (id: string, patch: Partial<Hotkey>) => void;
  onRemove: (id: string) => void;
  onAdded: (hotkey: Hotkey) => void;
  onError: (message: string) => void;
}) {
  const t = useT();
  return (
    <Group label={t('window.controls')} note={t('window.controlsHint')}>
      <div className="item">
        <div className="item__body">
          <span className="item__title">{t('window.modifierTapActions')}</span>
          <span className="item__desc">
            {keyboardEnabled
              ? t('window.modifierTapActionsHint')
              : t('window.modifierTapActionsDisabled')}
          </span>
        </div>
        {onOpenKeyboard && (
          <button type="button" className="btn btn--ghost" onClick={onOpenKeyboard}>
            {t('window.openKeyboard')}
          </button>
        )}
      </div>
      {hotkeys.length === 0 && <p className="empty-row">{t('window.noShortcuts')}</p>}
      {hotkeys.map((hotkey) => (
        <HotkeyRow
          key={hotkey.id}
          hotkey={hotkey}
          saving={savingHotkeyIds.has(hotkey.id)}
          onAccelerator={(accelerator) => onSave(hotkey.id, { accelerator })}
          onToggle={() => onSave(hotkey.id, { enabled: !hotkey.enabled })}
          onDelete={() => onRemove(hotkey.id)}
        />
      ))}
      <AddHotkeyForm options={shortcutOptions(t)} onAdded={onAdded} onError={onError} />
    </Group>
  );
}

function MouseControls({
  dragToSnapEnabled,
  dragToMoveEnabled,
  onDragToSnap,
  onDragToMove,
}: {
  dragToSnapEnabled: boolean;
  dragToMoveEnabled: boolean;
  onDragToSnap: (enabled: boolean) => void;
  onDragToMove: (enabled: boolean) => void;
}) {
  const t = useT();
  return (
    <Group label={t('window.mouse')}>
      <SwitchRow
        title={t('window.dragToSnapToggle')}
        desc={t('window.dragToSnapHint')}
        checked={dragToSnapEnabled}
        onChange={onDragToSnap}
        toggleLabel={t('window.enableDragToSnap')}
      />
      <SwitchRow
        title={t('window.dragToMoveToggle')}
        desc={t('window.dragToMoveHint')}
        checked={dragToMoveEnabled}
        onChange={onDragToMove}
        toggleLabel={t('window.enableDragToMove')}
      />
    </Group>
  );
}

function PlacementStage({
  context,
  history,
  activeSlot,
  busy,
  onRefresh,
  onRecall,
  onMove,
  onUndo,
  onRedo,
}: {
  context: PlacementContext | null;
  history: WindowHistoryStatus;
  activeSlot: PlacementSlot | null;
  busy: boolean;
  onRefresh: () => void;
  onRecall: () => void;
  onMove: () => void;
  onUndo: () => void;
  onRedo: () => void;
}) {
  const t = useT();
  const initial = context?.application.name.trim().charAt(0).toLocaleUpperCase() || '–';
  const remembered = context?.placements.length ?? 0;
  return (
    <section className="placement-stage">
      <div className="placement-stage__identity">
        <span className="placement-stage__initial" aria-hidden="true">
          {initial}
        </span>
        <div>
          <span className="placement-stage__eyebrow">{t('window.focusedApp')}</span>
          <strong title={context?.application.bundleId}>
            {context?.application.name ?? t('window.noFocusedApp')}
          </strong>
          {context && <small>{t('window.rememberedCount', { count: remembered })}</small>}
        </div>
        <button type="button" className="btn btn--ghost" onClick={onRefresh} disabled={busy}>
          {t('common.refresh')}
        </button>
      </div>
      <WorkAreaPreview context={context} activeSlot={activeSlot} />
      <div className="placement-stage__actions">
        <button
          type="button"
          className="btn btn--amber"
          onClick={onRecall}
          disabled={busy || !context || context.placements.length === 0}
        >
          {t('window.restoreHome')}
        </button>
        <button
          type="button"
          className="btn"
          onClick={onMove}
          disabled={
            busy || !context || !context.canMoveToDisplay || context.placements.length === 0
          }
        >
          {t('window.moveAndRestore')}
        </button>
        <span className="placement-stage__history">
          <button
            type="button"
            className="btn btn--ghost"
            onClick={onUndo}
            disabled={busy || !history.canUndo}
          >
            {t('common.undo')}
          </button>
          <button
            type="button"
            className="btn btn--ghost"
            onClick={onRedo}
            disabled={busy || !history.canRedo}
          >
            {t('common.redo')}
          </button>
        </span>
      </div>
    </section>
  );
}

function WorkAreaPreview({
  context,
  activeSlot,
}: {
  context: PlacementContext | null;
  activeSlot: PlacementSlot | null;
}) {
  const t = useT();
  return (
    <figure
      className="work-area-wrap"
      aria-label={
        context
          ? t('window.previewAria', {
              app: context.application.name,
              count: context.placements.length,
            })
          : t('window.previewEmptyAria')
      }
    >
      <div className="work-area" aria-hidden="true">
        {context?.placements.map((placement) => (
          <PreviewFrame
            key={placement.slot}
            frame={placement.frame}
            className={`work-area__home work-area__home--${placement.slot} ${
              activeSlot === placement.slot ? 'work-area__home--active' : ''
            }`}
          />
        ))}
        {context && <PreviewFrame frame={context.currentFrame} className="work-area__current" />}
      </div>
      <div className="work-area__legend" aria-hidden="true">
        <span>
          <i className="legend-mark legend-mark--current" />
          {t('window.currentPosition')}
        </span>
        <span>
          <i className="legend-mark legend-mark--primary" />
          {t('window.slot.primary')}
        </span>
        <span>
          <i className="legend-mark legend-mark--secondary" />
          {t('window.slot.secondary')}
        </span>
      </div>
    </figure>
  );
}

function PreviewFrame({ frame, className }: { frame: NormalizedRect; className: string }) {
  return (
    <span
      className={className}
      style={{
        left: `${frame.x * 100}%`,
        top: `${frame.y * 100}%`,
        width: `${frame.width * 100}%`,
        height: `${frame.height * 100}%`,
      }}
    />
  );
}

function PlacementSlotCard({
  slot,
  placement,
  active,
  busy,
  onCapture,
  onForget,
}: {
  slot: PlacementSlot;
  placement?: WindowPlacement;
  active: boolean;
  busy: boolean;
  onCapture: () => void;
  onForget: () => void;
}) {
  const t = useT();
  const [confirmingForget, setConfirmingForget] = useState(false);

  function forget() {
    if (confirmingForget) {
      setConfirmingForget(false);
      onForget();
    } else {
      setConfirmingForget(true);
    }
  }

  return (
    <article
      className={`placement-slot ${placement ? 'placement-slot--set' : ''} ${active ? 'placement-slot--active' : ''}`}
      aria-current={active ? 'true' : undefined}
    >
      <div className="placement-slot__preview" aria-hidden="true">
        {placement && <PreviewFrame frame={placement.frame} className="placement-slot__frame" />}
      </div>
      <div className="placement-slot__copy">
        <strong>{t(`window.slot.${slot}`)}</strong>
        <span>
          {active
            ? t('window.lastRestored')
            : placement
              ? t('window.homeReady')
              : t('window.homeEmpty')}
        </span>
      </div>
      <div className="placement-slot__actions">
        <button type="button" className="btn" onClick={onCapture} disabled={busy}>
          {placement ? t('window.replaceHome') : t('window.rememberHere')}
        </button>
        {placement && (
          <button
            type="button"
            className={`btn btn--ghost ${confirmingForget ? 'btn--warn' : ''}`}
            onClick={forget}
            onBlur={() => setConfirmingForget(false)}
            onKeyDown={(event) => {
              if (event.key === 'Escape') setConfirmingForget(false);
            }}
            disabled={busy}
            aria-label={
              confirmingForget
                ? t('window.confirmForgetAria', { slot: t(`window.slot.${slot}`) })
                : t('window.forgetAria', { slot: t(`window.slot.${slot}`) })
            }
          >
            {confirmingForget ? t('window.confirmForget') : t('common.forget')}
          </button>
        )}
      </div>
    </article>
  );
}
