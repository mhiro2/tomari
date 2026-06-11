import { useEffect, useState } from 'react';

import { ShortcutRecorder } from '../components/ShortcutRecorder';
import { EntityRow, Group, MasterSwitchHeader, Slider, Toggle, ValueRow } from '../components/ui';
import * as api from '../lib/api';
import { formatCmdError } from '../lib/errors';
import { actionLabel, modifierLabel, modifierWithSide, presetLabel } from '../lib/format';
import { useT, type Translator } from '../lib/i18n';
import { useSettings } from '../lib/settings';
import type { AppAction, Hotkey, ModifierRule, WindowPreset } from '../lib/types';

const SNAP_PRESETS: WindowPreset[] = ['leftHalf', 'rightHalf', 'maximize', 'center'];

const HOLD_MIN = 100;
const HOLD_MAX = 500;

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

/** Hold-role description for a modifier rule (hyper takes precedence over remap). */
function holdRoleLabel(rule: ModifierRule, t: Translator): string | null {
  if (rule.hyper) return t('keyboard.actsAsHyper');
  if (rule.remapTo) return t('keyboard.actsAs', { modifier: modifierLabel(rule.remapTo) });
  return null;
}

export function KeyboardView() {
  const t = useT();
  const { settings, update, updateSlider } = useSettings();
  const [rules, setRules] = useState<ModifierRule[]>([]);
  const [hotkeys, setHotkeys] = useState<Hotkey[]>([]);
  const [shortcutError, setShortcutError] = useState<string | null>(null);

  useEffect(() => {
    void api.listModifierRules().then(setRules);
    void api.listHotkeys().then(setHotkeys);
  }, []);

  async function toggleRule(rule: ModifierRule) {
    const next = { ...rule, enabled: !rule.enabled };
    await api.saveModifierRule(next);
    setRules((rs) => rs.map((r) => (r.id === rule.id ? next : r)));
  }

  async function saveHotkeyPatch(hk: Hotkey, patch: Partial<Hotkey>) {
    const next = { ...hk, ...patch };
    try {
      await api.saveHotkey(next);
      setHotkeys((hs) => hs.map((h) => (h.id === hk.id ? next : h)));
      setShortcutError(null);
    } catch (e) {
      setShortcutError(formatCmdError(e, t));
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
        <Group label={t('keyboard.modifierKeys')} note={t('keyboard.modifierHint')}>
          {rules.map((rule) => (
            <ModifierRow key={rule.id} rule={rule} onToggle={() => void toggleRule(rule)} />
          ))}
          <ValueRow
            title={t('keyboard.holdThreshold')}
            desc={t('keyboard.holdThresholdDesc')}
            trail={<span className="slider__value">{settings.holdThresholdMs} ms</span>}
          >
            <Slider
              value={settings.holdThresholdMs}
              min={HOLD_MIN}
              max={HOLD_MAX}
              step={10}
              onChange={(v) => updateSlider({ holdThresholdMs: v })}
              minLabel={`${HOLD_MIN} ms`}
              maxLabel={`${HOLD_MAX} ms`}
              ariaLabel={t('keyboard.holdThreshold')}
            />
          </ValueRow>
        </Group>

        <Group
          label={t('keyboard.globalShortcuts')}
          note={shortcutError ? <span className="hint--err">{shortcutError}</span> : undefined}
        >
          {hotkeys.map((hk) => (
            <HotkeyRow
              key={hk.id}
              hotkey={hk}
              onAccelerator={(accel) => void saveHotkeyPatch(hk, { accelerator: accel })}
              onToggle={() => void saveHotkeyPatch(hk, { enabled: !hk.enabled })}
              onDelete={() => void removeHotkey(hk.id)}
            />
          ))}
          <AddHotkeyForm onAdded={addHotkey} onError={setShortcutError} />
        </Group>
      </div>
    </div>
  );
}

function ModifierRow({ rule, onToggle }: { rule: ModifierRule; onToggle: () => void }) {
  const t = useT();
  return (
    <EntityRow
      lead={<div className="kbd-chip">{modifierWithSide(rule.modifier, rule.side, t)}</div>}
      title={rule.label}
      sub={
        <>
          {t('keyboard.tap')} → {actionLabel(rule.tap, t)}
          {holdRoleLabel(rule, t) ? ` · ${holdRoleLabel(rule, t)}` : ''}
        </>
      }
      trail={
        <Toggle
          checked={rule.enabled}
          onChange={onToggle}
          label={t('common.enable', { label: rule.label })}
        />
      }
    />
  );
}

function HotkeyRow({
  hotkey,
  onAccelerator,
  onToggle,
  onDelete,
}: {
  hotkey: Hotkey;
  onAccelerator: (accelerator: string) => void;
  onToggle: () => void;
  onDelete: () => void;
}) {
  const t = useT();
  return (
    <EntityRow
      lead={
        <ShortcutRecorder
          value={hotkey.accelerator}
          onCapture={onAccelerator}
          ariaLabel={t('keyboard.changeShortcut', { label: hotkey.label })}
        />
      }
      title={hotkey.label}
      sub={actionLabel(hotkey.action, t)}
      trail={
        <>
          <button
            type="button"
            className="btn btn--ghost"
            onClick={onDelete}
            aria-label={t('common.delete')}
          >
            ✕
          </button>
          <Toggle
            checked={hotkey.enabled}
            onChange={onToggle}
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
