import { listen } from '@tauri-apps/api/event';
import { useEffect, useEffectEvent, useLayoutEffect, useRef, useState } from 'react';

import { AddHotkeyForm, HotkeyRow, type HotkeyActionOption } from '../components/HotkeyEditor';
import { Banner, EntityRow, Group, MasterSwitchHeader, Toggle } from '../components/ui';
import * as api from '../lib/api';
import { formatCmdError } from '../lib/errors';
import { actionLabel, modifierLabel, modifierWithSide } from '../lib/format';
import { useT, type Translator } from '../lib/i18n';
import { useSettings } from '../lib/settings';
import type { AppAction, Hotkey, ModifierRule, PermissionsChanged } from '../lib/types';

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

type TapActionKey = 'none' | 'panel' | 'restoreWindow' | 'preventSleep' | 'menuBar';

function tapActionKey(action: AppAction): TapActionKey | 'custom' {
  switch (action.type) {
    case 'noOp':
      return 'none';
    case 'togglePanel':
      return 'panel';
    case 'recallWindowPlacement':
      return 'restoreWindow';
    case 'toggleKeepAwake':
      return 'preventSleep';
    case 'toggleMenuBar':
      return 'menuBar';
    default:
      return 'custom';
  }
}

function tapAction(key: TapActionKey): AppAction {
  switch (key) {
    case 'none':
      return { type: 'noOp' };
    case 'panel':
      return { type: 'togglePanel' };
    case 'restoreWindow':
      return { type: 'recallWindowPlacement' };
    case 'preventSleep':
      return { type: 'toggleKeepAwake' };
    case 'menuBar':
      return { type: 'toggleMenuBar' };
  }
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

// `onOpenSetup` swaps the tabs for the permission-setup checklist; the banner
// only offers it when the shell provides one (it renders standalone in tests).
export function KeyboardView({ onOpenSetup }: { onOpenSetup?: () => void }) {
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
  // Mirrored on commit (never during render, which React can replay or
  // discard) so the refs only ever hold values from committed renders. Layout
  // timing, not passive: the commit that stores a save's result also
  // re-enables the row's controls, so the refs must be current before the
  // next click can possibly be handled.
  useLayoutEffect(() => {
    rulesRef.current = rules;
    hotkeysRef.current = hotkeys;
  }, [rules, hotkeys]);
  // Formats through the current `t` without the mount-only effect below
  // depending on it — `useT()` returns a new closure on every render, so
  // adding it to the effect's deps would re-run the fetch each time.
  const formatLoadError = useEffectEvent((e: unknown) => formatCmdError(e, t));

  useEffect(() => {
    void api
      .listModifierRules()
      .then(setRules)
      .catch((e: unknown) => setModifierError(formatLoadError(e)));
    void api
      .listHotkeys()
      .then(setHotkeys)
      .catch((e: unknown) => setShortcutError(formatLoadError(e)));
    void api
      .inputMonitoringStatus()
      .then(setInputMonitoringGranted)
      .catch((e: unknown) => setShortcutError(formatLoadError(e)));
    // Accessibility/Input Monitoring are granted in System Settings, outside
    // the app, so follow the backend's poll rather than requiring a reopen.
    const unlisten = listen<PermissionsChanged>('tomari:permissions-changed', (e) =>
      setInputMonitoringGranted(e.payload.inputMonitoring),
    );
    return () => void unlisten.then((fn) => fn());
  }, []);

  async function saveRulePatch(id: string, patch: Partial<ModifierRule>) {
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
    const next = { ...current, ...patch };
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
  const keyboardHotkeys = hotkeys.filter((hotkey) => !isWindowAction(hotkey.action));
  const shortcutOptions: HotkeyActionOption[] = [
    { key: 'togglePanel', label: t('action.togglePanel'), action: { type: 'togglePanel' } },
    {
      key: 'toggleKeepAwake',
      label: t('action.toggleKeepAwake'),
      action: { type: 'toggleKeepAwake' },
    },
    {
      key: 'toggleMenuBar',
      label: t('action.toggleMenuBar'),
      action: { type: 'toggleMenuBar' },
    },
  ];

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
            {onOpenSetup && (
              <button type="button" className="btn btn--ghost" onClick={onOpenSetup}>
                {t('setup.openSetup')}
              </button>
            )}
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
              onToggle={() => void saveRulePatch(rule.id, { enabled: !rule.enabled })}
              onTapAction={(action) => void saveRulePatch(rule.id, { tap: action })}
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
          {keyboardHotkeys.length === 0 && <p className="hint">{t('keyboard.noHotkeys')}</p>}
          {keyboardHotkeys.map((hk) => (
            <HotkeyRow
              key={hk.id}
              hotkey={hk}
              saving={savingHotkeyIds.has(hk.id)}
              onAccelerator={(accel) => void saveHotkeyPatch(hk.id, { accelerator: accel })}
              onToggle={() => void saveHotkeyPatch(hk.id, { enabled: !hk.enabled })}
              onDelete={() => void removeHotkey(hk.id)}
            />
          ))}
          <AddHotkeyForm options={shortcutOptions} onAdded={addHotkey} onError={setShortcutError} />
        </Group>
      </div>
    </div>
  );
}

function ModifierRow({
  rule,
  saving,
  onToggle,
  onTapAction,
}: {
  rule: ModifierRule;
  saving: boolean;
  onToggle: () => void;
  onTapAction: (action: AppAction) => void;
}) {
  const t = useT();
  const selected = tapActionKey(rule.tap);
  return (
    <EntityRow
      lead={<div className="kbd-chip">{modifierWithSide(rule.modifier, rule.side, t)}</div>}
      title={modifierLabel(rule.modifier)}
      sub={
        <span className="modifier-rule__details">
          <span>{modifierDesc(rule, t)}</span>
          <label>
            <span>{t('keyboard.tapAction')}</span>
            <select
              className="input input--compact"
              value={selected}
              onChange={(event) => onTapAction(tapAction(event.target.value as TapActionKey))}
              disabled={saving}
              aria-label={t('keyboard.tapActionFor', { modifier: modifierLabel(rule.modifier) })}
            >
              {selected === 'custom' && (
                <option value="custom" disabled>
                  {actionLabel(rule.tap, t)}
                </option>
              )}
              <option value="none">{t('action.noOp')}</option>
              <option value="panel">{t('action.togglePanel')}</option>
              <option value="restoreWindow">{t('action.recallPlacement')}</option>
              <option value="preventSleep">{t('action.toggleKeepAwake')}</option>
              <option value="menuBar">{t('action.toggleMenuBar')}</option>
            </select>
          </label>
        </span>
      }
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
