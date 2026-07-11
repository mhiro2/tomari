import { listen } from '@tauri-apps/api/event';
import { useEffect, useRef, useState } from 'react';

import { ShortcutRecorder } from '../components/ShortcutRecorder';
import { Banner, EntityRow, Group, MasterSwitchHeader, Toggle } from '../components/ui';
import * as api from '../lib/api';
import { formatCmdError } from '../lib/errors';
import { actionLabel, modifierLabel, modifierWithSide, presetLabel } from '../lib/format';
import { useT, type Translator } from '../lib/i18n';
import { useSettings } from '../lib/settings';
import type {
  AppAction,
  Hotkey,
  ModifierRule,
  PermissionsChanged,
  WindowPreset,
} from '../lib/types';

const SNAP_PRESETS: WindowPreset[] = ['leftHalf', 'rightHalf', 'maximize', 'center'];

const WINDOW_ACTIONS = [
  { key: 'moveDisplayNext', action: { type: 'moveWindowToDisplay', value: 'next' } },
  { key: 'moveDisplayPrev', action: { type: 'moveWindowToDisplay', value: 'prev' } },
  { key: 'undoWindow', action: { type: 'undoWindow' } },
] satisfies { key: string; action: AppAction }[];

function actionFromKey(key: string): AppAction {
  if (key === 'togglePanel') return { type: 'togglePanel' };
  if (key === 'toggleKeepAwake') return { type: 'toggleKeepAwake' };
  const windowAction = WINDOW_ACTIONS.find((a) => a.key === key);
  if (windowAction) return windowAction.action;
  return { type: 'snapWindow', value: key as WindowPreset };
}

function ActionOptions({ t }: { t: Translator }) {
  return (
    <>
      <option value="togglePanel">{t('action.togglePanel')}</option>
      <option value="toggleKeepAwake">{t('action.toggleKeepAwake')}</option>
      {SNAP_PRESETS.map((p) => (
        <option key={p} value={p}>
          {t('action.snap', { target: presetLabel(p, t) })}
        </option>
      ))}
      {WINDOW_ACTIONS.map((a) => (
        <option key={a.key} value={a.key}>
          {actionLabel(a.action, t)}
        </option>
      ))}
    </>
  );
}

/** One-line description of what a modifier rule does, derived from the rule
 * itself (not a stored label) so it reads naturally in either language. */
function modifierDesc(rule: ModifierRule, t: Translator): string {
  const hasTap = rule.tap.type !== 'noOp';
  if (rule.hyper) {
    return hasTap
      ? t('keyboard.tapHold', { action: actionLabel(rule.tap, t), modifier: 'Hyper (⌃⌥⇧⌘)' })
      : t('keyboard.usedAsHyper');
  }
  if (rule.remapTo) {
    const modifier = modifierLabel(rule.remapTo);
    return hasTap
      ? t('keyboard.tapHold', { action: actionLabel(rule.tap, t), modifier })
      : t('keyboard.usedAs', { modifier });
  }
  return hasTap ? t('keyboard.tapFor', { action: actionLabel(rule.tap, t) }) : '';
}

// Removes a single id from a "saving" set — shared by the save functions
// below so a save (successful, failed, or skipped) always clears its flag.
function clearSaving(
  setIds: (fn: (s: ReadonlySet<string>) => ReadonlySet<string>) => void,
  id: string,
) {
  setIds((s) => {
    const rest = new Set(s);
    rest.delete(id);
    return rest;
  });
}

export function KeyboardView() {
  const t = useT();
  const { settings, update } = useSettings();
  const [rules, setRules] = useState<ModifierRule[]>([]);
  const [hotkeys, setHotkeys] = useState<Hotkey[]>([]);
  const [shortcutError, setShortcutError] = useState<string | null>(null);
  const [modifierError, setModifierError] = useState<string | null>(null);
  const [inputMonitoringGranted, setInputMonitoringGranted] = useState(true);
  // Ids with a save in flight, so their row's controls can be disabled — this
  // both prevents a second click racing the first save and, since the base
  // for a patch is always read from these refs (not a render-captured prop),
  // ensures a queued edit is applied on top of the latest persisted value
  // rather than clobbering it.
  const [savingRuleIds, setSavingRuleIds] = useState<ReadonlySet<string>>(new Set());
  const [savingHotkeyIds, setSavingHotkeyIds] = useState<ReadonlySet<string>>(new Set());
  const rulesRef = useRef(rules);
  const hotkeysRef = useRef(hotkeys);
  rulesRef.current = rules;
  hotkeysRef.current = hotkeys;
  // Mirrors `t` so the mount-only effect below can format a load failure
  // without depending on `t` itself — `useT()` returns a new closure on every
  // render, so adding it to the effect's deps would re-run the fetch each time.
  const tRef = useRef(t);
  tRef.current = t;

  useEffect(() => {
    void api
      .listModifierRules()
      .then(setRules)
      .catch((e: unknown) => setModifierError(formatCmdError(e, tRef.current)));
    void api
      .listHotkeys()
      .then(setHotkeys)
      .catch((e: unknown) => setShortcutError(formatCmdError(e, tRef.current)));
    void api
      .inputMonitoringStatus()
      .then(setInputMonitoringGranted)
      .catch((e: unknown) => setShortcutError(formatCmdError(e, tRef.current)));
    // Accessibility/Input Monitoring are granted in System Settings, outside
    // the app, so follow the backend's poll rather than requiring a reopen.
    const unlisten = listen<PermissionsChanged>('tomari:permissions-changed', (e) =>
      setInputMonitoringGranted(e.payload.inputMonitoring),
    );
    return () => void unlisten.then((fn) => fn());
  }, []);

  async function toggleRule(id: string) {
    // A second click while the first save is still in flight must not fire
    // another save — the row is disabled while saving, but guard here too
    // against any event queued just before that took effect.
    if (savingRuleIds.has(id)) return;
    setSavingRuleIds((s) => new Set(s).add(id));
    // Read the base from the latest state, not a value captured by the
    // caller's render, so this can't undo an edit that landed in between.
    const current = rulesRef.current.find((r) => r.id === id);
    if (!current) {
      clearSaving(setSavingRuleIds, id);
      return;
    }
    const next = { ...current, enabled: !current.enabled };
    // Only reflect the toggle in the UI once the backend has persisted it and
    // reloaded the engine — a save failure must surface rather than leave the
    // row showing a state the runtime never adopted.
    try {
      await api.saveModifierRule(next);
      setRules((rs) => rs.map((r) => (r.id === id ? next : r)));
      setModifierError(null);
    } catch (e) {
      setModifierError(formatCmdError(e, t));
    } finally {
      clearSaving(setSavingRuleIds, id);
    }
  }

  async function saveHotkeyPatch(id: string, patch: Partial<Hotkey>) {
    if (savingHotkeyIds.has(id)) return;
    setSavingHotkeyIds((s) => new Set(s).add(id));
    // Read the base from the latest state, not a value captured by the
    // caller's render, so an accelerator saved while an enabled-toggle is
    // still in flight (or vice versa) is applied on top of it, not over it.
    const current = hotkeysRef.current.find((h) => h.id === id);
    if (!current) {
      clearSaving(setSavingHotkeyIds, id);
      return;
    }
    const next = { ...current, ...patch };
    try {
      await api.saveHotkey(next);
      setHotkeys((hs) => hs.map((h) => (h.id === id ? next : h)));
      setShortcutError(null);
    } catch (e) {
      setShortcutError(formatCmdError(e, t));
    } finally {
      clearSaving(setSavingHotkeyIds, id);
    }
  }

  async function removeHotkey(id: string) {
    try {
      await api.deleteHotkey(id);
      setHotkeys((hs) => hs.filter((h) => h.id !== id));
      setShortcutError(null);
    } catch (e) {
      setShortcutError(formatCmdError(e, t));
    }
  }

  function addHotkey(hk: Hotkey) {
    setHotkeys((hs) => [...hs, hk]);
    setShortcutError(null);
  }

  async function grantInputMonitoring() {
    try {
      const ok = await api.requestInputMonitoring();
      setInputMonitoringGranted(ok);
    } catch (e) {
      setShortcutError(formatCmdError(e, t));
    }
  }

  if (!settings) return <div className="view">{t('common.loading')}</div>;

  const on = settings.keyboardEnabled;

  return (
    <div className="view">
      <MasterSwitchHeader
        title={t('settings.keyboardCustomization')}
        checked={on}
        onChange={(v) => update({ keyboardEnabled: v })}
        offNote={t('keyboard.offNote')}
        enableLabel={t('common.turnOn')}
        toggleLabel={t('common.enable', { label: t('settings.keyboardCustomization') })}
      />

      <div className={`view ${on ? '' : 'gated'}`} aria-disabled={!on} inert={!on}>
        {!inputMonitoringGranted && (
          <Banner tone="warn">
            <div className="banner__body">
              <strong>{t('keyboard.imNeeded')}</strong>
              <p>{t('keyboard.imBody')}</p>
            </div>
            <button
              type="button"
              className="btn btn--primary"
              onClick={() => void grantInputMonitoring()}
            >
              {t('window.grantAccess')}
            </button>
          </Banner>
        )}

        <Group
          label={t('keyboard.modifierKeys')}
          note={
            modifierError ? (
              <span className="hint--err" role="alert">
                {modifierError}
              </span>
            ) : (
              t('keyboard.modifierHint')
            )
          }
        >
          {rules.length === 0 && <p className="hint">{t('keyboard.noModifierRules')}</p>}
          {rules.map((rule) => (
            <ModifierRow
              key={rule.id}
              rule={rule}
              saving={savingRuleIds.has(rule.id)}
              onToggle={() => void toggleRule(rule.id)}
            />
          ))}
          <EntityRow
            lead={<div className="kbd-chip">⌘</div>}
            title={t('keyboard.commandImeSwitch')}
            sub={t('keyboard.commandImeSwitchDesc')}
            trail={
              <Toggle
                checked={settings.commandImeSwitchEnabled}
                onChange={(v) => update({ commandImeSwitchEnabled: v })}
                label={t('common.enable', { label: t('keyboard.commandImeSwitch') })}
              />
            }
          />
        </Group>

        <Group
          label={t('keyboard.globalShortcuts')}
          note={
            shortcutError ? (
              <span className="hint--err" role="alert">
                {shortcutError}
              </span>
            ) : undefined
          }
        >
          {hotkeys.length === 0 && <p className="hint">{t('keyboard.noHotkeys')}</p>}
          {hotkeys.map((hk) => (
            <HotkeyRow
              key={hk.id}
              hotkey={hk}
              saving={savingHotkeyIds.has(hk.id)}
              onAccelerator={(accel) => void saveHotkeyPatch(hk.id, { accelerator: accel })}
              onToggle={() => void saveHotkeyPatch(hk.id, { enabled: !hk.enabled })}
              onDelete={() => void removeHotkey(hk.id)}
            />
          ))}
          <AddHotkeyForm onAdded={addHotkey} onError={setShortcutError} />
        </Group>
      </div>
    </div>
  );
}

function ModifierRow({
  rule,
  saving,
  onToggle,
}: {
  rule: ModifierRule;
  saving: boolean;
  onToggle: () => void;
}) {
  const t = useT();
  return (
    <EntityRow
      lead={<div className="kbd-chip">{modifierWithSide(rule.modifier, rule.side, t)}</div>}
      title={modifierLabel(rule.modifier)}
      sub={modifierDesc(rule, t)}
      trail={
        <Toggle
          checked={rule.enabled}
          onChange={onToggle}
          disabled={saving}
          label={t('common.enable', { label: modifierLabel(rule.modifier) })}
        />
      }
    />
  );
}

function HotkeyRow({
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
  // A bare ✕ right next to the enable toggle is one misclick away from
  // deleting a hotkey with no way back, so the first click only arms a
  // confirmation — the actual delete needs a second, deliberate click.
  const [confirming, setConfirming] = useState(false);

  function handleDeleteClick() {
    if (confirming) {
      onDelete();
      setConfirming(false);
    } else {
      setConfirming(true);
    }
  }

  return (
    <EntityRow
      lead={
        // ShortcutRecorder has no disabled prop of its own; `inert` keeps it
        // out of the tab order and blocks pointer/keyboard input while a save
        // for this row is in flight, the same technique used for the gated
        // master-switch content above.
        <span inert={saving}>
          <ShortcutRecorder
            value={hotkey.accelerator}
            onCapture={onAccelerator}
            ariaLabel={t('keyboard.changeShortcut', { label: hotkey.label })}
          />
        </span>
      }
      title={hotkey.label}
      sub={actionLabel(hotkey.action, t)}
      trail={
        <>
          <button
            type="button"
            className={`btn btn--ghost ${confirming ? 'btn--amber' : ''}`}
            onClick={handleDeleteClick}
            onBlur={() => setConfirming(false)}
            onKeyDown={(e) => {
              if (e.key === 'Escape' && confirming) {
                e.stopPropagation();
                setConfirming(false);
              }
            }}
            disabled={saving}
            aria-label={
              confirming
                ? t('common.deleteConfirm', { label: hotkey.label })
                : t('keyboard.deleteShortcut', { label: hotkey.label })
            }
          >
            {confirming ? t('common.deleteConfirmShort') : '✕'}
          </button>
          <Toggle
            checked={hotkey.enabled}
            onChange={onToggle}
            disabled={saving}
            label={t('common.enable', { label: hotkey.label })}
          />
        </>
      }
    />
  );
}

function AddHotkeyForm({
  onAdded,
  onError,
}: {
  onAdded: (hk: Hotkey) => void;
  onError: (msg: string) => void;
}) {
  const t = useT();
  const [label, setLabel] = useState('');
  const [accelerator, setAccelerator] = useState('');
  const [actionKey, setActionKey] = useState('togglePanel');
  const [busy, setBusy] = useState(false);

  // The recorder only emits backend-normalized accelerators.
  const canSubmit = label.trim() !== '' && accelerator !== '' && !busy;

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    if (!canSubmit) return;
    setBusy(true);
    try {
      const hk: Hotkey = {
        id: `hk-${crypto.randomUUID()}`,
        label: label.trim(),
        accelerator,
        action: actionFromKey(actionKey),
        enabled: true,
      };
      await api.saveHotkey(hk);
      onAdded(hk);
      setLabel('');
      setAccelerator('');
    } catch (e) {
      onError(formatCmdError(e, t));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form className="add-form" onSubmit={(e) => void submit(e)}>
      <input
        className="input"
        placeholder={t('common.label')}
        value={label}
        onChange={(e) => setLabel(e.target.value)}
        aria-label={t('keyboard.shortcutLabelAria')}
      />
      <ShortcutRecorder
        value={accelerator}
        onCapture={setAccelerator}
        ariaLabel={t('keyboard.recordShortcut')}
      />
      <select
        className="input"
        value={actionKey}
        onChange={(e) => setActionKey(e.target.value)}
        aria-label={t('keyboard.actionAria')}
      >
        <ActionOptions t={t} />
      </select>
      <button type="submit" className="btn btn--primary" disabled={!canSubmit}>
        {t('common.add')}
      </button>
    </form>
  );
}
