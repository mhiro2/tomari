import { listen } from '@tauri-apps/api/event';
import { useEffect, useEffectEvent, useLayoutEffect, useRef, useState } from 'react';

import { AddHotkeyForm, type HotkeyActionOption } from '../components/HotkeyEditor';
import { ShortcutRecorder } from '../components/ShortcutRecorder';
import {
  FeatureContent,
  FeaturePageHeader,
  FeatureSwitch,
  SegmentedPageNav,
  SettingsList,
  Toggle,
} from '../components/ui';
import * as api from '../lib/api';
import { formatCmdError } from '../lib/errors';
import { actionLabel, presetLabel, safeDisplayText } from '../lib/format';
import { useT, type Translator } from '../lib/i18n';
import { useSettings } from '../lib/settings';
import type {
  AppAction,
  HistoryActionResult,
  Hotkey,
  NormalizedRect,
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
const EMPTY_KEYCAPS: readonly string[] = [];
const PRIMARY_SHORTCUT_COUNT = 5;
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

type StatusSource = 'contextLoad' | 'other';
type Status = {
  message: string;
  isError: boolean;
  undoPlacement: boolean;
  source: StatusSource;
};
type WindowTab = 'saved' | 'shortcuts' | 'mouse';

const WINDOW_TABS: WindowTab[] = ['saved', 'shortcuts', 'mouse'];

function shortcutRank(action: AppAction): number {
  if (action.type === 'snapWindow' || action.type === 'snapWindowExact') {
    switch (action.value) {
      case 'leftHalf':
        return 0;
      case 'rightHalf':
        return 1;
      case 'maximize':
        return 2;
      default:
        return 20 + WINDOW_PRESETS.indexOf(action.value);
    }
  }
  switch (action.type) {
    case 'recallWindowPlacement':
      return 3;
    case 'moveWindowToDisplayAndRecall':
      return action.value === 'next' ? 4 : 30;
    case 'moveWindowToDisplay':
      return action.value === 'next' ? 31 : 32;
    case 'undoWindow':
      return 40;
    case 'redoWindow':
      return 41;
    default:
      return 100;
  }
}

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
    ...WINDOW_PRESETS.map((preset): HotkeyActionOption => ({
      key: preset,
      label: t('action.snap', { target: presetLabel(preset, t) }),
      action: { type: 'snapWindow', value: preset },
    })),
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

export function WindowView({ onOpenKeyboard }: { onOpenKeyboard?: () => void }) {
  const t = useT();
  const { settings, update } = useSettings();
  const [context, setContext] = useState<PlacementContext | null>(null);
  const [history, setHistory] = useState<WindowHistoryStatus>(EMPTY_HISTORY);
  const [activeSlot, setActiveSlot] = useState<PlacementSlot | null>(null);
  const [hotkeys, setHotkeys] = useState<Hotkey[]>([]);
  const [tab, setTab] = useState<WindowTab>('saved');
  const [showOtherShortcuts, setShowOtherShortcuts] = useState(false);
  const [showAddShortcut, setShowAddShortcut] = useState(false);
  const [status, setStatus] = useState<Status | null>(null);
  const [busy, setBusy] = useState(false);
  const [savingHotkeyIds, setSavingHotkeyIds] = useState<ReadonlySet<string>>(new Set());
  const clearTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const contextRequestRef = useRef<Promise<PlacementContext> | null>(null);
  // Bumped by every post-mutation pull. A context request that started before
  // a mutation describes the world before it; its result must not land on top
  // of the pull that followed the mutation, however late it arrives.
  const contextGenerationRef = useRef(0);
  const hotkeysRef = useRef(hotkeys);

  useLayoutEffect(() => {
    hotkeysRef.current = hotkeys;
  }, [hotkeys]);

  function showStatus(
    message: string,
    isError: boolean,
    undoPlacement = false,
    source: StatusSource = 'other',
  ) {
    if (clearTimerRef.current !== null) clearTimeout(clearTimerRef.current);
    setStatus({ message, isError, undoPlacement, source });
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
  const reportContextLoadError = useEffectEvent((error: unknown) =>
    showStatus(formatCmdError(error, t), true, false, 'contextLoad'),
  );

  // The generation a refresh belongs to: a new one after a mutation, so every
  // pull issued before it is stale from here on; the current one otherwise.
  function refreshGeneration(afterMutation: boolean): number {
    if (afterMutation) contextGenerationRef.current += 1;
    return contextGenerationRef.current;
  }

  // `afterMutation`: the pull follows a change this view just made. It must
  // then be a *new* request — one already in flight was issued against the
  // pre-mutation state and may answer with it — and every older request's
  // result is discarded when it lands. Focus/show refreshes (`false`) still
  // coalesce onto whatever pull is in flight. `generation` lets a caller that
  // refreshes several things at once hand them the same one.
  async function refreshContext(
    resetActive: boolean,
    reportError = true,
    afterMutation = false,
    generation = refreshGeneration(afterMutation),
  ) {
    const request = (afterMutation ? null : contextRequestRef.current) ?? api.getPlacementContext();
    contextRequestRef.current = request;
    try {
      const next = await request;
      if (generation !== contextGenerationRef.current) return null;
      setContext(next);
      if (resetActive) setActiveSlot(null);
      setStatus((current) => (current?.source === 'contextLoad' ? null : current));
      return next;
    } catch (error) {
      if (generation !== contextGenerationRef.current) return null;
      setContext(null);
      setActiveSlot(null);
      if (reportError) reportContextLoadError(error);
      return null;
    } finally {
      if (contextRequestRef.current === request) contextRequestRef.current = null;
    }
  }

  // Same staleness rule as `refreshContext`: undo/redo change the history, so
  // a status pull that started before them must not overwrite the one after.
  async function refreshHistory(reportError = true, generation = contextGenerationRef.current) {
    try {
      const next = await api.getWindowHistoryStatus();
      if (generation !== contextGenerationRef.current) return null;
      const available = next ?? EMPTY_HISTORY;
      setHistory(available);
      return available;
    } catch (error) {
      if (generation !== contextGenerationRef.current) return null;
      setHistory(EMPTY_HISTORY);
      if (reportError) reportLoadError(error);
      return null;
    }
  }

  async function refreshWorkflow(resetActive: boolean, reportError = true, afterMutation = false) {
    const generation = refreshGeneration(afterMutation);
    const [nextContext] = await Promise.all([
      refreshContext(resetActive, reportError, afterMutation, generation),
      refreshHistory(reportError, generation),
    ]);
    return nextContext;
  }

  const refreshForPanelFocus = useEffectEvent(() => {
    void refreshWorkflow(true);
  });

  useEffect(() => {
    refreshForPanelFocus();
    void api
      .listHotkeys()
      .then((items) => setHotkeys(items.filter((item) => isWindowAction(item.action))))
      .catch(reportLoadError);

    const panelUnlisten = listen('tomari:panel-shown', refreshForPanelFocus);
    const onFocus = () => refreshForPanelFocus();
    window.addEventListener('focus', onFocus);
    return () => {
      if (clearTimerRef.current !== null) clearTimeout(clearTimerRef.current);
      window.removeEventListener('focus', onFocus);
      void panelUnlisten.then((fn) => fn());
    };
  }, []);

  async function runTargetAction<T>(action: (target: WindowTarget) => Promise<T>) {
    if (busy || !context) return null;
    const snapshot = context;
    setBusy(true);
    try {
      const result = await action(snapshot.target);
      const nextContext = await refreshWorkflow(false, false, true);
      if (!sameTarget(snapshot.target, nextContext?.target)) setActiveSlot(null);
      return { result, snapshot, nextContext };
    } catch (error) {
      showStatus(formatCmdError(error, t), true);
      if (isWindowTargetChanged(error)) await refreshWorkflow(true, false, true);
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
      outcome.result.undoable,
    );
  }

  async function forget(slot: PlacementSlot) {
    const outcome = await runTargetAction((target) => api.forgetWindowPlacement(target, slot));
    if (!outcome) return;
    showStatus(
      t('window.forgotten', { slot: t(`window.slot.${slot}`) }),
      false,
      outcome.result.undoable,
    );
  }

  async function undoPlacementEdit() {
    if (busy) return;
    setBusy(true);
    try {
      const result = await api.undoWindowPlacementEdit();
      // The whole workflow, not just the context: bumping the generation
      // discards any history pull in flight, so history must be re-pulled too.
      await refreshWorkflow(false, false, true);
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
      await refreshWorkflow(false, false, true);
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
      const saved = await api.saveHotkey(next);
      setHotkeys((items) => items.map((item) => (item.id === id ? saved : item)));
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

  if (!settings) return <div className="view">{t('common.loading')}</div>;

  const on = settings.windowManagementEnabled;
  const orderedHotkeys = hotkeys.toSorted(
    (left, right) => shortcutRank(left.action) - shortcutRank(right.action),
  );
  const primaryHotkeys = orderedHotkeys.slice(0, PRIMARY_SHORTCUT_COUNT);
  const otherHotkeys = orderedHotkeys.slice(PRIMARY_SHORTCUT_COUNT);

  return (
    <div className="view">
      <FeaturePageHeader title={t('app.nav.window')} description={t('window.pageDescription')} />

      <FeatureSwitch
        title={t('common.enable', { label: t('settings.windowManagement') })}
        checked={on}
        onChange={(value) => update({ windowManagementEnabled: value })}
        stateLabel={on ? t('common.on') : t('common.off')}
        tone={on ? 'on' : 'neutral'}
      />

      <SegmentedPageNav
        label={t('window.tabsLabel')}
        idBase="window-tabs"
        value={tab}
        onChange={setTab}
        items={WINDOW_TABS.map((value) => ({ value, label: t(`window.tab.${value}`) }))}
      />

      <FeatureContent enabled={on}>
        <div
          id="window-tabs-panel"
          className="window-tab-panel"
          role="tabpanel"
          aria-labelledby={`window-tabs-${tab}-tab`}
        >
          {tab === 'saved' && (
            <WindowSavedPanel
              context={context}
              history={history}
              activeSlot={activeSlot}
              busy={busy}
              onRefresh={() => void refreshWorkflow(true)}
              onRecall={() => void recall()}
              onMove={() => void moveAndRecall()}
              onUndo={() => void changeHistory(api.undoWindow, 'undo')}
              onRedo={() => void changeHistory(api.redoWindow, 'redo')}
              onCapture={(slot) => void capture(slot)}
              onForget={(slot) => void forget(slot)}
            />
          )}
          {tab === 'shortcuts' && (
            <WindowShortcutsPanel
              keyboardEnabled={settings.keyboardEnabled}
              onOpenKeyboard={onOpenKeyboard}
              primaryHotkeys={primaryHotkeys}
              otherHotkeys={otherHotkeys}
              savingHotkeyIds={savingHotkeyIds}
              showOther={showOtherShortcuts}
              onShowOther={setShowOtherShortcuts}
              showAdd={showAddShortcut}
              onShowAdd={setShowAddShortcut}
              onSave={(id, patch) => void saveHotkeyPatch(id, patch)}
              onRemove={(id) => void removeHotkey(id)}
              onAdded={(hotkey) => setHotkeys((items) => [...items, hotkey])}
              onError={(message) => showStatus(message, true)}
            />
          )}
          {tab === 'mouse' && (
            <MouseControls
              dragToSnapEnabled={settings.dragToSnapEnabled}
              dragToMoveEnabled={settings.dragToMoveEnabled}
              onDragToSnap={(value) => update({ dragToSnapEnabled: value })}
              onDragToMove={(value) => update({ dragToMoveEnabled: value })}
            />
          )}
        </div>
      </FeatureContent>

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

function WindowSavedPanel({
  context,
  history,
  activeSlot,
  busy,
  onRefresh,
  onRecall,
  onMove,
  onUndo,
  onRedo,
  onCapture,
  onForget,
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
  onCapture: (slot: PlacementSlot) => void;
  onForget: (slot: PlacementSlot) => void;
}) {
  return (
    <div className="window-saved">
      <PlacementStage
        context={context}
        history={history}
        activeSlot={activeSlot}
        busy={busy}
        onRefresh={onRefresh}
        onRecall={onRecall}
        onMove={onMove}
        onUndo={onUndo}
        onRedo={onRedo}
      />
      <RememberedPositions
        context={context}
        activeSlot={activeSlot}
        busy={busy}
        onCapture={onCapture}
        onForget={onForget}
      />
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
    <section className="window-saved__positions">
      <header className="settings-section-heading">
        <h2>{t('window.rememberedHomes')}</h2>
      </header>
      <div className="placement-slots">
        {(['primary', 'secondary'] as const).map((slot) => (
          <PlacementSlotCard
            key={slot}
            slot={slot}
            placement={context?.placements.find((placement) => placement.slot === slot)}
            damaged={context?.damagedPlacements.includes(slot) ?? false}
            active={activeSlot === slot}
            busy={busy || !context}
            onCapture={() => onCapture(slot)}
            onForget={() => onForget(slot)}
          />
        ))}
      </div>
    </section>
  );
}

function WindowShortcutsPanel({
  keyboardEnabled,
  onOpenKeyboard,
  primaryHotkeys,
  otherHotkeys,
  savingHotkeyIds,
  showOther,
  onShowOther,
  showAdd,
  onShowAdd,
  onSave,
  onRemove,
  onAdded,
  onError,
}: {
  keyboardEnabled: boolean;
  onOpenKeyboard?: () => void;
  primaryHotkeys: Hotkey[];
  otherHotkeys: Hotkey[];
  savingHotkeyIds: ReadonlySet<string>;
  showOther: boolean;
  onShowOther: (show: boolean) => void;
  showAdd: boolean;
  onShowAdd: (show: boolean) => void;
  onSave: (id: string, patch: Partial<Hotkey>) => void;
  onRemove: (id: string) => void;
  onAdded: (hotkey: Hotkey) => void;
  onError: (message: string) => void;
}) {
  const t = useT();

  useEffect(() => {
    if (!keyboardEnabled && showAdd) onShowAdd(false);
  }, [keyboardEnabled, onShowAdd, showAdd]);

  const shortcutRows = (items: Hotkey[]) =>
    items.map((hotkey) => (
      <WindowShortcutRow
        key={hotkey.id}
        hotkey={hotkey}
        saving={savingHotkeyIds.has(hotkey.id)}
        onAccelerator={(accelerator) => onSave(hotkey.id, { accelerator })}
        onToggle={() => onSave(hotkey.id, { enabled: !hotkey.enabled })}
        onDelete={() => onRemove(hotkey.id)}
      />
    ));

  return (
    <section className="window-shortcuts">
      <FeatureContent enabled={keyboardEnabled}>
        <div className="window-shortcuts">
          <header className="settings-section-heading">
            <h2>{t('window.basicShortcuts')}</h2>
            <button type="button" className="btn" onClick={() => onShowAdd(true)}>
              {t('window.addShortcut')}
            </button>
          </header>

          <SettingsList>
            {primaryHotkeys.length === 0 && otherHotkeys.length === 0 ? (
              <p className="empty-row">{t('window.noShortcuts')}</p>
            ) : (
              shortcutRows(primaryHotkeys)
            )}
            {showOther && shortcutRows(otherHotkeys)}
            {otherHotkeys.length > 0 && (
              <button
                type="button"
                className="settings-list__disclosure"
                aria-expanded={showOther}
                onClick={() => onShowOther(!showOther)}
              >
                {showOther
                  ? t('window.hideMoreShortcuts')
                  : t('window.moreShortcuts', { count: otherHotkeys.length })}
              </button>
            )}
          </SettingsList>
        </div>
      </FeatureContent>

      <div className="window-shortcuts__keyboard-link">
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

      {showAdd && keyboardEnabled && (
        <AddWindowShortcutDialog
          options={shortcutOptions(t)}
          onDismiss={() => onShowAdd(false)}
          onAdded={(hotkey) => {
            onAdded(hotkey);
            onShowAdd(false);
          }}
          onError={onError}
        />
      )}
    </section>
  );
}

function AddWindowShortcutDialog({
  options,
  onDismiss,
  onAdded,
  onError,
}: {
  options: HotkeyActionOption[];
  onDismiss: () => void;
  onAdded: (hotkey: Hotkey) => void;
  onError: (message: string) => void;
}) {
  const t = useT();
  const dialogRef = useRef<HTMLDialogElement>(null);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    if (typeof dialog.showModal === 'function') {
      dialog.showModal();
    } else {
      dialog.setAttribute('open', '');
    }
    dialog.querySelector<HTMLInputElement>('input')?.focus();

    return () => {
      if (typeof dialog.close === 'function' && dialog.open) {
        dialog.close();
      } else {
        dialog.removeAttribute('open');
      }
    };
  }, []);

  return (
    <dialog
      ref={dialogRef}
      className="settings-sheet"
      aria-labelledby="window-add-shortcut-title"
      onCancel={(event) => {
        event.preventDefault();
        onDismiss();
      }}
    >
      <header className="settings-sheet__header">
        <h2 id="window-add-shortcut-title">{t('window.addShortcut')}</h2>
        <button type="button" className="btn btn--ghost" onClick={onDismiss}>
          {t('common.cancel')}
        </button>
      </header>
      <AddHotkeyForm options={options} onAdded={onAdded} onError={onError} />
    </dialog>
  );
}

function WindowShortcutRow({
  hotkey,
  saving,
  onAccelerator,
  onToggle,
  onDelete,
}: {
  hotkey: Hotkey;
  saving: boolean;
  onAccelerator: (accelerator: string) => void;
  onToggle: () => void;
  onDelete: () => void;
}) {
  const t = useT();
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const action = safeDisplayText(actionLabel(hotkey.action, t));
  const label = safeDisplayText(hotkey.label);
  const title = !label || label === action ? action : `${label} — ${action}`;

  function deleteHotkey() {
    if (confirmingDelete) {
      setConfirmingDelete(false);
      onDelete();
    } else {
      setConfirmingDelete(true);
    }
  }

  return (
    <div className="window-shortcut-row">
      <WindowActionIcon action={hotkey.action} />
      <span className="window-shortcut-row__title">{title}</span>
      <span inert={saving}>
        <ShortcutRecorder
          value={safeDisplayText(hotkey.accelerator)}
          onCapture={onAccelerator}
          ariaLabel={t('keyboard.changeShortcut', { label: title })}
        />
      </span>
      <button
        type="button"
        className={`btn btn--ghost ${confirmingDelete ? 'btn--warn' : ''}`}
        onClick={deleteHotkey}
        onBlur={() => setConfirmingDelete(false)}
        onKeyDown={(event) => {
          if (event.key === 'Escape') setConfirmingDelete(false);
        }}
        disabled={saving}
        aria-label={
          confirmingDelete
            ? t('common.deleteConfirm', { label: title })
            : t('keyboard.deleteShortcut', { label: title })
        }
      >
        {confirmingDelete ? t('common.deleteConfirmShort') : '✕'}
      </button>
      <Toggle
        checked={hotkey.enabled}
        onChange={onToggle}
        disabled={saving}
        label={t('common.enable', { label: title })}
      />
    </div>
  );
}

function WindowActionIcon({ action }: { action: AppAction }) {
  let kind: string = action.type;
  let mark = '↔';
  if (action.type === 'snapWindow' || action.type === 'snapWindowExact') {
    kind = `snap-${action.value}`;
    mark = '';
  } else if (action.type === 'recallWindowPlacement') {
    mark = '⌑';
  } else if (action.type === 'moveWindowToDisplayAndRecall') {
    mark = action.value === 'next' ? '→⌑' : '←⌑';
  } else if (action.type === 'moveWindowToDisplay') {
    mark = action.value === 'next' ? '→' : '←';
  } else if (action.type === 'undoWindow') {
    mark = '↶';
  } else if (action.type === 'redoWindow') {
    mark = '↷';
  }
  return (
    <span className={`window-action-icon window-action-icon--${kind}`} aria-hidden="true">
      {mark}
    </span>
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
    <section className="window-mouse">
      <header className="settings-section-heading">
        <h2>{t('window.mouse')}</h2>
      </header>
      <div className="window-mouse__grid">
        <MouseGestureCard
          visual="snap"
          title={t('window.dragGesture')}
          description={t('window.dragToSnapHint')}
          checked={dragToSnapEnabled}
          onChange={onDragToSnap}
          toggleLabel={t('window.enableDragToSnap')}
        />
        <MouseGestureCard
          visual="move-resize"
          keycaps={['⌃', '⌥']}
          title={t('window.resizeGesture')}
          description={t('window.dragToMoveHint')}
          checked={dragToMoveEnabled}
          onChange={onDragToMove}
          toggleLabel={t('window.enableDragToMove')}
        />
      </div>
    </section>
  );
}

function MouseGestureCard({
  visual,
  keycaps = EMPTY_KEYCAPS,
  title,
  description,
  checked,
  onChange,
  toggleLabel,
}: {
  visual: 'snap' | 'move-resize';
  keycaps?: readonly string[];
  title: string;
  description: string;
  checked: boolean;
  onChange: (enabled: boolean) => void;
  toggleLabel: string;
}) {
  return (
    <article className={`mouse-gesture mouse-gesture--${visual}`}>
      <div className="mouse-gesture__visual" aria-hidden="true">
        {keycaps.map((keycap) => (
          <kbd key={keycap}>{keycap}</kbd>
        ))}
        <span className="mouse-gesture__pointer" />
        <span className="mouse-gesture__window" />
      </div>
      <div className="mouse-gesture__copy">
        <strong>{title}</strong>
        <span>{description}</span>
      </div>
      <Toggle checked={checked} onChange={onChange} label={toggleLabel} />
    </article>
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
    <section className="window-saved__current">
      <header className="settings-section-heading">
        <h2>{t('window.currentWindow')}</h2>
        <button type="button" className="btn btn--ghost" onClick={onRefresh} disabled={busy}>
          {t('common.refresh')}
        </button>
      </header>
      <div className="placement-stage">
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
  damaged,
  active,
  busy,
  onCapture,
  onForget,
}: {
  slot: PlacementSlot;
  placement?: WindowPlacement;
  // A stored row exists for this slot but cannot be used; the slot is offered
  // for replacing or forgetting rather than shown as empty.
  damaged: boolean;
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
              : damaged
                ? t('window.homeDamaged')
                : t('window.homeEmpty')}
        </span>
      </div>
      <div className="placement-slot__actions">
        <button type="button" className="btn" onClick={onCapture} disabled={busy}>
          {placement || damaged ? t('window.replaceHome') : t('window.rememberHere')}
        </button>
        {(placement || damaged) && (
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
