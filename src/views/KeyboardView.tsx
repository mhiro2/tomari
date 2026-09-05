import { useEffect, useEffectEvent, useLayoutEffect, useRef, useState } from 'react';

import { AddHotkeyForm, HotkeyRow, type HotkeyActionOption } from '../components/HotkeyEditor';
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
import { actionLabel, modifierLabel, modifierWithSide, safeDisplayText } from '../lib/format';
import { useT, type MessageKey, type Translator } from '../lib/i18n';
import { useSettings } from '../lib/settings';
import type {
  AppAction,
  ConfigurationIssue,
  ConfigurationIssueReason,
  ConfigurationWarnings,
  Hotkey,
  ModifierRule,
} from '../lib/types';

const CONFIGURATION_REASON_KEYS = {
  emptyId: 'keyboard.configurationIssueReason.emptyId',
  idTooLong: 'keyboard.configurationIssueReason.idTooLong',
  emptyLabel: 'keyboard.configurationIssueReason.emptyLabel',
  labelTooLong: 'keyboard.configurationIssueReason.labelTooLong',
  invalidAccelerator: 'keyboard.configurationIssueReason.invalidAccelerator',
  unsafeGlobalShortcut: 'keyboard.configurationIssueReason.unsafeGlobalShortcut',
  invalidKeystroke: 'keyboard.configurationIssueReason.invalidKeystroke',
  reservedRuleId: 'keyboard.configurationIssueReason.reservedRuleId',
  hyperWithRemap: 'keyboard.configurationIssueReason.hyperWithRemap',
  reservedCommandSlot: 'keyboard.configurationIssueReason.reservedCommandSlot',
  duplicateId: 'keyboard.configurationIssueReason.duplicateId',
  duplicateAccelerator: 'keyboard.configurationIssueReason.duplicateAccelerator',
  duplicateModifierSlot: 'keyboard.configurationIssueReason.duplicateModifierSlot',
} as const satisfies Record<ConfigurationIssueReason, MessageKey>;

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

export type KeyboardTab = 'modifiers' | 'shortcuts';

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

// The apply-warning codes a modifier-rule save or delete reports on; the
// shared warning state replaces exactly these.
const RULE_MUTATION_PROBES = ['capsLockRemap'] as const;

export function KeyboardView({ initialTab = 'modifiers' }: { initialTab?: KeyboardTab }) {
  const t = useT();
  const { settings, configurationWarnings, update, reportApplyOutcome } = useSettings();
  const [rules, setRules] = useState<ModifierRule[]>([]);
  const [hotkeys, setHotkeys] = useState<Hotkey[]>([]);
  const [rulesLoaded, setRulesLoaded] = useState(false);
  const [hotkeysLoaded, setHotkeysLoaded] = useState(false);
  const [shortcutError, setShortcutError] = useState<string | null>(null);
  const [modifierError, setModifierError] = useState<string | null>(null);
  const [configurationError, setConfigurationError] = useState<string | null>(null);
  const [tab, setTab] = useState<KeyboardTab>(initialTab);
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
      const outcome = await api.saveModifierRule(next);
      setRules((rs) => rs.map((r) => (r.id === id ? outcome.rule : r)));
      setModifierError(null);
      // The rule is stored and live; whether the Caps Lock HID remap followed
      // is what the outcome reports. It goes into the shared warning state
      // (shown by the app-level banner) so it outlives this page and clears
      // on the next clean apply, from whichever save that comes.
      reportApplyOutcome(outcome, RULE_MUTATION_PROBES);
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
      const saved = await api.saveHotkey(next);
      setHotkeys((hs) => hs.map((h) => (h.id === id ? saved : h)));
      setShortcutError(null);
    } catch (e) {
      setShortcutError(formatCmdError(e, t));
    } finally {
      clearSaving(setSavingHotkeyIds, id);
    }
  }

  async function removeHotkey(id: string, fromConfigurationWarning = false) {
    if (savingHotkeyIds.has(id)) return;
    setSavingHotkeyIds((ids) => new Set(ids).add(id));
    try {
      await api.deleteHotkey(id);
      setHotkeys((hs) => hs.filter((h) => h.id !== id));
      setShortcutError(null);
      setConfigurationError(null);
    } catch (e) {
      const message = formatCmdError(e, t);
      if (fromConfigurationWarning) {
        setConfigurationError(message);
      } else {
        setShortcutError(message);
      }
    } finally {
      clearSaving(setSavingHotkeyIds, id);
    }
  }

  async function removeInvalidModifierRule(id: string) {
    if (savingRuleIds.has(id)) return;
    setSavingRuleIds((ids) => new Set(ids).add(id));
    try {
      const outcome = await api.deleteModifierRule(id);
      setRules((items) => items.filter((rule) => rule.id !== id));
      setConfigurationError(null);
      setModifierError(null);
      reportApplyOutcome(outcome, RULE_MUTATION_PROBES);
    } catch (error) {
      setConfigurationError(formatCmdError(error, t));
    } finally {
      clearSaving(setSavingRuleIds, id);
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
      />

      {configurationWarnings !== null &&
        configurationWarnings.invalidHotkeys.length +
          configurationWarnings.invalidModifierRules.length >
          0 && (
          <ConfigurationIssues
            warnings={configurationWarnings}
            deletingHotkeyIds={savingHotkeyIds}
            deletingModifierRuleIds={savingRuleIds}
            error={configurationError}
            onDeleteHotkey={(id) => void removeHotkey(id, true)}
            onDeleteModifierRule={(id) => void removeInvalidModifierRule(id)}
          />
        )}

      <FeatureSwitch
        title={t('common.enable', { label: t('settings.keyboardCustomization') })}
        checked={on}
        onChange={(v) => update({ keyboardEnabled: v })}
        stateLabel={on ? t('common.on') : t('common.off')}
        tone={on ? 'on' : 'neutral'}
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

function configurationIssueLabel(issue: ConfigurationIssue, t: Translator): string {
  const label = safeDisplayText(issue.label);
  if (label) return label;
  const id = safeDisplayText(issue.id);
  return id
    ? t('keyboard.configurationIssueUnnamed', { id })
    : t('keyboard.configurationIssueUnknown');
}

function ConfigurationIssueGroup({
  title,
  issues,
  deletingIds,
  onDelete,
}: {
  title: string;
  issues: ConfigurationIssue[];
  deletingIds: ReadonlySet<string>;
  onDelete: (id: string) => void;
}) {
  const t = useT();
  if (issues.length === 0) return null;
  return (
    <section className="configuration-issues__group">
      <h3>{title}</h3>
      <ul>
        {issues.map((issue) => {
          const label = configurationIssueLabel(issue, t);
          return (
            <li key={`${issue.id}:${issue.reason}`}>
              <span className="configuration-issues__item-copy">
                <strong>{label}</strong>
                <span>{t(CONFIGURATION_REASON_KEYS[issue.reason])}</span>
              </span>
              <ConfigurationIssueDeleteButton
                label={label}
                deleting={deletingIds.has(issue.id)}
                onDelete={() => onDelete(issue.id)}
              />
            </li>
          );
        })}
      </ul>
    </section>
  );
}

function ConfigurationIssueDeleteButton({
  label,
  deleting,
  onDelete,
}: {
  label: string;
  deleting: boolean;
  onDelete: () => void;
}) {
  const t = useT();
  const [confirming, setConfirming] = useState(false);
  return (
    <button
      type="button"
      className={`btn btn--ghost ${confirming ? 'btn--warn' : ''}`}
      onClick={(event) => {
        if (confirming) {
          setConfirming(false);
          onDelete();
          return;
        }
        setConfirming(true);
        // macOS WebKit does not focus a button on click. Explicit focus makes
        // the blur/Escape cancellation contract work for mouse users too.
        event.currentTarget.focus();
      }}
      onBlur={() => setConfirming(false)}
      onKeyDown={(event) => {
        if (event.key === 'Escape') setConfirming(false);
      }}
      disabled={deleting}
      aria-label={
        confirming
          ? t('common.deleteConfirm', { label })
          : t('keyboard.deleteInvalidItem', { label })
      }
    >
      {confirming ? t('common.deleteConfirmShort') : t('common.delete')}
    </button>
  );
}

function ConfigurationIssues({
  warnings,
  deletingHotkeyIds,
  deletingModifierRuleIds,
  error,
  onDeleteHotkey,
  onDeleteModifierRule,
}: {
  warnings: ConfigurationWarnings;
  deletingHotkeyIds: ReadonlySet<string>;
  deletingModifierRuleIds: ReadonlySet<string>;
  error: string | null;
  onDeleteHotkey: (id: string) => void;
  onDeleteModifierRule: (id: string) => void;
}) {
  const t = useT();
  return (
    <section
      className="configuration-issues"
      aria-labelledby="keyboard-configuration-issues-title"
      aria-describedby="keyboard-configuration-issues-description"
    >
      <header className="configuration-issues__header">
        <h2 id="keyboard-configuration-issues-title" tabIndex={-1}>
          {t('keyboard.configurationIssuesTitle')}
        </h2>
        <p id="keyboard-configuration-issues-description">
          {t('keyboard.configurationIssuesBody')}
        </p>
      </header>
      <div className="configuration-issues__groups">
        <ConfigurationIssueGroup
          title={t('keyboard.configurationIssuesModifierRules')}
          issues={warnings.invalidModifierRules}
          deletingIds={deletingModifierRuleIds}
          onDelete={onDeleteModifierRule}
        />
        <ConfigurationIssueGroup
          title={t('keyboard.configurationIssuesHotkeys')}
          issues={warnings.invalidHotkeys}
          deletingIds={deletingHotkeyIds}
          onDelete={onDeleteHotkey}
        />
      </div>
      {error && (
        <p className="hint hint--err" role="alert">
          {error}
        </p>
      )}
    </section>
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
              {safeDisplayText(actionLabel(rule.tap, t))}
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
