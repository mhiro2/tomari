import { useEffect, useEffectEvent, useLayoutEffect, useRef, useState } from 'react';

import { AddHotkeyForm, HotkeyRow, type HotkeyActionOption } from '../components/HotkeyEditor';
import {
  FeatureContent,
  FeaturePageHeader,
  SegmentedPageNav,
  SettingsList,
  Toggle,
} from '../components/ui';
import * as api from '../lib/api';
import { formatCmdError } from '../lib/errors';
import { actionLabel, modifierLabel, modifierWithSide } from '../lib/format';
import { useT, type Translator } from '../lib/i18n';
import { useSettings } from '../lib/settings';
import type { AppAction, Hotkey, ModifierRule } from '../lib/types';

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

/** The role a key keeps while held or used in a chord. */
function heldModifierLabel(rule: ModifierRule, t: Translator): string {
  if (rule.hyper) {
    return t('keyboard.usedAsHyper');
  }
  return t('keyboard.usedAs', {
    modifier: modifierLabel(rule.remapTo ?? rule.modifier),
  });
}

type KeyboardTab = 'modifiers' | 'shortcuts';

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

export function KeyboardView() {
  const t = useT();
  const { settings, update } = useSettings();
  const [rules, setRules] = useState<ModifierRule[]>([]);
  const [hotkeys, setHotkeys] = useState<Hotkey[]>([]);
  const [rulesLoaded, setRulesLoaded] = useState(false);
  const [hotkeysLoaded, setHotkeysLoaded] = useState(false);
  const [shortcutError, setShortcutError] = useState<string | null>(null);
  const [modifierError, setModifierError] = useState<string | null>(null);
  const [tab, setTab] = useState<KeyboardTab>('modifiers');
  const [addingShortcut, setAddingShortcut] = useState(false);
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
      .catch((e: unknown) => setModifierError(formatLoadError(e)))
      .finally(() => setRulesLoaded(true));
    void api
      .listHotkeys()
      .then(setHotkeys)
      .catch((e: unknown) => setShortcutError(formatLoadError(e)))
      .finally(() => setHotkeysLoaded(true));
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
    setAddingShortcut(false);
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
    <div className="view keyboard-view">
      <FeaturePageHeader
        title={t('app.nav.keyboard')}
        description={t('keyboard.pageDescription')}
        checked={on}
        onChange={(v) => update({ keyboardEnabled: v })}
        toggleLabel={t('common.enable', { label: t('settings.keyboardCustomization') })}
        onLabel={t('common.on')}
        offLabel={t('common.off')}
      />

      <SegmentedPageNav
        label={t('keyboard.tabsLabel')}
        idBase="keyboard-tabs"
        value={tab}
        onChange={setTab}
        items={[
          { value: 'modifiers', label: t('keyboard.tab.modifiers') },
          { value: 'shortcuts', label: t('keyboard.tab.shortcuts') },
        ]}
      />

      <FeatureContent enabled={on}>
        {tab === 'modifiers' ? (
          <ModifiersPanel
            rules={rules}
            loaded={rulesLoaded}
            error={modifierError}
            savingIds={savingRuleIds}
            commandImeSwitchEnabled={settings.commandImeSwitchEnabled}
            onToggle={(rule) => void saveRulePatch(rule.id, { enabled: !rule.enabled })}
            onTapAction={(rule, action) => void saveRulePatch(rule.id, { tap: action })}
            onCommandImeSwitch={(enabled) => update({ commandImeSwitchEnabled: enabled })}
          />
        ) : (
          <ShortcutsPanel
            hotkeys={keyboardHotkeys}
            loaded={hotkeysLoaded}
            error={shortcutError}
            savingIds={savingHotkeyIds}
            options={shortcutOptions}
            adding={addingShortcut}
            onStartAdding={() => setAddingShortcut(true)}
            onCancelAdding={() => setAddingShortcut(false)}
            onSave={(id, patch) => void saveHotkeyPatch(id, patch)}
            onRemove={(id) => void removeHotkey(id)}
            onAdded={addHotkey}
            onError={setShortcutError}
          />
        )}
      </FeatureContent>
    </div>
  );
}

function ModifiersPanel({
  rules,
  loaded,
  error,
  savingIds,
  commandImeSwitchEnabled,
  onToggle,
  onTapAction,
  onCommandImeSwitch,
}: {
  rules: ModifierRule[];
  loaded: boolean;
  error: string | null;
  savingIds: ReadonlySet<string>;
  commandImeSwitchEnabled: boolean;
  onToggle: (rule: ModifierRule) => void;
  onTapAction: (rule: ModifierRule, action: AppAction) => void;
  onCommandImeSwitch: (enabled: boolean) => void;
}) {
  const t = useT();
  return (
    <div
      id="keyboard-tabs-panel"
      className="keyboard-panel"
      role="tabpanel"
      aria-labelledby="keyboard-tabs-modifiers-tab"
    >
      <section className="keyboard-section">
        <header className="keyboard-section__header">
          <h2 id="keyboard-modifiers-title">{t('keyboard.modifierKeys')}</h2>
        </header>
        <SettingsList>
          {!loaded && <p className="empty-row">{t('common.loading')}</p>}
          {loaded && rules.length === 0 && (
            <p className="empty-row">{t('keyboard.noModifierRules')}</p>
          )}
          {rules.length > 0 && (
            <table className="modifier-table">
              <thead>
                <tr>
                  <th scope="col">{t('keyboard.table.key')}</th>
                  <th scope="col">{t('keyboard.table.tap')}</th>
                  <th scope="col">{t('keyboard.table.hold')}</th>
                  <th scope="col">{t('keyboard.table.enabled')}</th>
                </tr>
              </thead>
              <tbody>
                {rules.map((rule) => (
                  <ModifierRow
                    key={rule.id}
                    rule={rule}
                    saving={savingIds.has(rule.id)}
                    onToggle={() => onToggle(rule)}
                    onTapAction={(action) => onTapAction(rule, action)}
                  />
                ))}
              </tbody>
            </table>
          )}
        </SettingsList>
        {error && (
          <p className="hint hint--err" role="alert">
            {error}
          </p>
        )}
      </section>

      <section className="keyboard-section" aria-labelledby="keyboard-ime-title">
        <header className="keyboard-section__header">
          <h2 id="keyboard-ime-title">{t('keyboard.inputSwitching')}</h2>
        </header>
        <SettingsList>
          <div className="command-ime">
            <div className="command-ime__map">
              <span className="command-ime__binding">
                <span className="kbd-chip">{t('keyboard.leftCommand')}</span>
                <span className="command-ime__arrow">→</span>
                <strong className="command-ime__target">{t('keyboard.imeEisu')}</strong>
              </span>
              <span className="command-ime__binding">
                <span className="kbd-chip">{t('keyboard.rightCommand')}</span>
                <span className="command-ime__arrow">→</span>
                <strong className="command-ime__target">{t('keyboard.imeKana')}</strong>
              </span>
            </div>
            <Toggle
              checked={commandImeSwitchEnabled}
              onChange={onCommandImeSwitch}
              label={t('common.enable', { label: t('keyboard.commandImeSwitch') })}
              describedBy="keyboard-ime-note"
            />
          </div>
        </SettingsList>
        <p className="hint" id="keyboard-ime-note">
          {t('keyboard.commandImeSwitchNote')}
        </p>
      </section>
    </div>
  );
}

function ShortcutsPanel({
  hotkeys,
  loaded,
  error,
  savingIds,
  options,
  adding,
  onStartAdding,
  onCancelAdding,
  onSave,
  onRemove,
  onAdded,
  onError,
}: {
  hotkeys: Hotkey[];
  loaded: boolean;
  error: string | null;
  savingIds: ReadonlySet<string>;
  options: HotkeyActionOption[];
  adding: boolean;
  onStartAdding: () => void;
  onCancelAdding: () => void;
  onSave: (id: string, patch: Partial<Hotkey>) => void;
  onRemove: (id: string) => void;
  onAdded: (hotkey: Hotkey) => void;
  onError: (message: string) => void;
}) {
  const t = useT();
  return (
    <section
      id="keyboard-tabs-panel"
      className="keyboard-panel"
      role="tabpanel"
      aria-labelledby="keyboard-tabs-shortcuts-tab"
    >
      <header className="keyboard-section__header keyboard-section__header--action">
        <h2 id="keyboard-shortcuts-title">{t('keyboard.globalShortcuts')}</h2>
        <button type="button" className="btn" onClick={onStartAdding} hidden={adding}>
          {t('keyboard.addShortcut')}
        </button>
      </header>
      <SettingsList>
        {!loaded && <p className="empty-row">{t('common.loading')}</p>}
        {loaded && hotkeys.length === 0 && <p className="empty-row">{t('keyboard.noHotkeys')}</p>}
        {hotkeys.map((hotkey) => (
          <HotkeyRow
            key={hotkey.id}
            hotkey={hotkey}
            saving={savingIds.has(hotkey.id)}
            onAccelerator={(accelerator) => onSave(hotkey.id, { accelerator })}
            onToggle={() => onSave(hotkey.id, { enabled: !hotkey.enabled })}
            onDelete={() => onRemove(hotkey.id)}
          />
        ))}
      </SettingsList>
      {adding && (
        <div className="keyboard-shortcut-builder">
          <AddHotkeyForm options={options} onAdded={onAdded} onError={onError} />
          <button
            type="button"
            className="btn btn--ghost"
            aria-label={t('keyboard.cancelAddShortcut')}
            onClick={onCancelAdding}
          >
            {t('common.cancel')}
          </button>
        </div>
      )}
      {error && (
        <p className="hint hint--err" role="alert">
          {error}
        </p>
      )}
    </section>
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
    <tr>
      <th scope="row">
        <span className="modifier-table__key">
          <span className="kbd-chip">{modifierWithSide(rule.modifier, rule.side, t)}</span>
          <span>{modifierLabel(rule.modifier)}</span>
        </span>
      </th>
      <td>
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
      </td>
      <td>
        <span className="modifier-table__hold">{heldModifierLabel(rule, t)}</span>
      </td>
      <td>
        <Toggle
          checked={rule.enabled}
          onChange={onToggle}
          disabled={saving}
          label={t('common.enable', { label: modifierLabel(rule.modifier) })}
        />
      </td>
    </tr>
  );
}
