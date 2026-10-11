import { useEffect, useId, useRef, useState, type RefObject } from 'react';

import * as api from '../lib/api';
import { formatCmdError } from '../lib/errors';
import { actionLabel, modifierLabel, physicalKeyLabel, safeDisplayText } from '../lib/format';
import { useT } from '../lib/i18n';
import { WINDOW_PRESETS } from '../lib/types';
import type {
  AppAction,
  KeySide,
  ModifierKey,
  ModifierRule,
  RuleMutationOutcome,
  SaveModifierRuleOutcome,
} from '../lib/types';
import { ShortcutRecorder } from './ShortcutRecorder';

const MODIFIERS: ModifierKey[] = ['capsLock', 'control', 'option', 'command', 'shift', 'function'];

function actionKey(action: AppAction): string {
  if (action.type === 'sendKeystroke') return action.type;
  return 'value' in action ? `${action.type}:${action.value}` : action.type;
}

function hasSides(modifier: ModifierKey): boolean {
  return modifier !== 'capsLock' && modifier !== 'function';
}

const PHYSICAL_KEYS = MODIFIERS.flatMap((modifier) => {
  const sides: KeySide[] = hasSides(modifier) ? ['left', 'right', 'either'] : ['either'];
  return sides.map((side) => ({ modifier, side, value: `${modifier}:${side}` }));
});

export function ModifierRuleEditor({
  rule,
  repairIdentity = false,
  menuBarSupported,
  onSaved,
  onDeleted,
  onClose,
}: {
  rule: ModifierRule | null;
  repairIdentity?: boolean;
  menuBarSupported: boolean;
  onSaved: (outcome: SaveModifierRuleOutcome, originalId: string | undefined) => void;
  onDeleted: (outcome: RuleMutationOutcome, originalId: string) => void;
  onClose: () => void;
}) {
  const t = useT();
  const titleId = useId();
  const dialogRef = useRef<HTMLDialogElement>(null);
  const keyRef = useRef<HTMLSelectElement>(null);
  // Keep raw editable values. Display sanitization must never rewrite a saved
  // rule merely because its editor was opened.
  const [draft, setDraft] = useState<ModifierRule>(
    () =>
      rule ?? {
        id: `rule-${crypto.randomUUID()}`,
        label: '',
        modifier: 'capsLock',
        side: 'either',
        remapTo: null,
        hyper: false,
        tap: { type: 'noOp' },
        enabled: true,
      },
  );
  const [replacementId] = useState(() => (repairIdentity ? `rule-${crypto.randomUUID()}` : null));
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    dialog.showModal();
    keyRef.current?.focus();
    return () => {
      if (dialog.open) dialog.close();
    };
  }, []);

  const reservedCommand = draft.modifier === 'command' && draft.side !== 'either';
  const invalidSide = !hasSides(draft.modifier) && draft.side !== 'either';
  const contradictoryHold = draft.hyper && draft.remapTo !== null && draft.remapTo !== undefined;
  const canSave =
    !busy &&
    !reservedCommand &&
    !invalidSide &&
    !contradictoryHold &&
    (draft.tap.type !== 'sendKeystroke' || draft.tap.value.trim() !== '');

  function patch(next: Partial<ModifierRule>) {
    setDraft((current) => ({ ...current, ...next }));
    setError(null);
    setConfirmDelete(false);
  }

  async function save(event: React.FormEvent) {
    event.preventDefault();
    if (!canSave || busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setError(null);
    try {
      const candidate = {
        ...draft,
        id: replacementId ?? draft.id,
        label: physicalKeyLabel(draft.modifier, draft.side, t),
      };
      const outcome = await api.saveModifierRule(candidate, rule?.id);
      onSaved(outcome, rule?.id);
    } catch (cause) {
      setError(formatCmdError(cause, t));
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  }

  async function remove() {
    if (!rule || busyRef.current) return;
    if (!confirmDelete) {
      setConfirmDelete(true);
      return;
    }
    busyRef.current = true;
    setBusy(true);
    setError(null);
    try {
      onDeleted(await api.deleteModifierRule(rule.id), rule.id);
    } catch (cause) {
      setError(formatCmdError(cause, t));
    } finally {
      busyRef.current = false;
      setBusy(false);
      setConfirmDelete(false);
    }
  }

  return (
    <dialog
      ref={dialogRef}
      className="settings-sheet modifier-editor"
      aria-labelledby={titleId}
      onCancel={(event) => {
        event.preventDefault();
        if (!busyRef.current) onClose();
      }}
    >
      <form onSubmit={(event) => void save(event)}>
        <header className="modifier-editor__header">
          <h2 id={titleId}>{t(rule ? 'keyboard.editRule' : 'keyboard.addRule')}</h2>
        </header>
        <ModifierRuleFields
          draft={draft}
          patch={patch}
          busy={busy}
          keyRef={keyRef}
          menuBarSupported={menuBarSupported}
        />
        {error && (
          <p className="hint hint--err" role="alert">
            {error}
          </p>
        )}
        <footer className="modifier-editor__actions">
          {rule && (
            <button
              type="button"
              className={`btn btn--ghost ${confirmDelete ? 'btn--warn' : ''}`}
              disabled={busy}
              onClick={(event) => {
                event.currentTarget.focus();
                void remove();
              }}
              onBlur={() => setConfirmDelete(false)}
              onKeyDown={(event) => {
                if (event.key === 'Escape' && confirmDelete) {
                  event.preventDefault();
                  event.stopPropagation();
                  setConfirmDelete(false);
                }
              }}
              aria-label={t(confirmDelete ? 'common.deleteConfirm' : 'keyboard.deleteRule', {
                label: safeDisplayText(rule.label),
              })}
            >
              {t(confirmDelete ? 'common.deleteConfirmShort' : 'common.delete')}
            </button>
          )}
          <span className="modifier-editor__actions-end">
            <button type="button" className="btn btn--ghost" disabled={busy} onClick={onClose}>
              {t('common.cancel')}
            </button>
            <button type="submit" className="btn btn--primary" disabled={!canSave}>
              {t(busy ? 'keyboard.savingRule' : 'keyboard.saveRule')}
            </button>
          </span>
        </footer>
      </form>
    </dialog>
  );
}

function ModifierRuleFields({
  draft,
  patch,
  busy,
  keyRef,
  menuBarSupported,
}: {
  draft: ModifierRule;
  patch: (next: Partial<ModifierRule>) => void;
  busy: boolean;
  keyRef: RefObject<HTMLSelectElement | null>;
  menuBarSupported: boolean;
}) {
  const t = useT();
  const hintId = useId();
  const reservedCommand = draft.modifier === 'command' && draft.side !== 'either';
  const invalidSide = !hasSides(draft.modifier) && draft.side !== 'either';
  const contradictoryHold = draft.hyper && draft.remapTo !== null && draft.remapTo !== undefined;
  const hold = contradictoryHold
    ? 'conflict'
    : draft.hyper
      ? 'hyper'
      : (draft.remapTo ?? 'original');
  const selectedAction = actionKey(draft.tap);
  const actions: AppAction[] = [
    { type: 'noOp' },
    { type: 'togglePanel' },
    { type: 'openDisplaySettings' },
    { type: 'resetDisplayDimming' },
    { type: 'recallWindowPlacement' },
    { type: 'undoWindow' },
    { type: 'redoWindow' },
    { type: 'moveWindowToDisplay', value: 'next' },
    { type: 'moveWindowToDisplay', value: 'prev' },
    { type: 'moveWindowToDisplayAndRecall', value: 'next' },
    { type: 'moveWindowToDisplayAndRecall', value: 'prev' },
    ...WINDOW_PRESETS.map((value): AppAction => ({ type: 'snapWindow', value })),
    { type: 'switchIme', value: 'alphanumeric' },
    { type: 'switchIme', value: 'kana' },
    { type: 'sendKeystroke', value: '' },
    { type: 'toggleKeepAwake' },
  ];
  if (menuBarSupported || draft.tap.type === 'toggleMenuBar')
    actions.push({ type: 'toggleMenuBar' });
  // Preserve an existing exact-snap action without quietly changing its cycle
  // semantics when some other field is edited.
  if (!actions.some((action) => actionKey(action) === selectedAction)) actions.push(draft.tap);

  return (
    <fieldset className="add-form modifier-editor__fields" disabled={busy}>
      <legend className="sr-only">{t('keyboard.ruleSettings')}</legend>
      <label className="add-form__field add-form__field--wide">
        <span>{t('keyboard.physicalKey')}</span>
        <select
          ref={keyRef}
          className="input"
          value={`${draft.modifier}:${draft.side}`}
          onChange={(event) => {
            const key = PHYSICAL_KEYS.find((candidate) => candidate.value === event.target.value);
            if (key) patch({ modifier: key.modifier, side: key.side });
          }}
        >
          {invalidSide && (
            <option value={`${draft.modifier}:${draft.side}`} disabled>
              {modifierLabel(draft.modifier)} — {t('keyboard.invalidPhysicalKey')}
            </option>
          )}
          {PHYSICAL_KEYS.map(({ modifier, side, value }) => (
            <option
              key={value}
              value={value}
              disabled={modifier === 'command' && side !== 'either'}
            >
              {physicalKeyLabel(modifier, side, t)}
            </option>
          ))}
        </select>
      </label>
      {(draft.modifier === 'command' || invalidSide) && (
        <p
          className="hint modifier-editor__note"
          role={reservedCommand || invalidSide ? 'alert' : undefined}
        >
          {t(
            invalidSide
              ? 'keyboard.configurationIssueReason.unsupportedModifierSide'
              : 'keyboard.reservedCommandHint',
          )}
        </p>
      )}
      <label className="add-form__field add-form__field--wide">
        <span>{t('keyboard.holdRole')}</span>
        <select
          className="input"
          value={hold}
          onChange={(event) => {
            const value = event.target.value;
            patch({
              hyper: value === 'hyper',
              remapTo: value === 'hyper' || value === 'original' ? null : (value as ModifierKey),
            });
          }}
        >
          {contradictoryHold && (
            <option value="conflict" disabled>
              {t('keyboard.chooseHoldRole')}
            </option>
          )}
          <option value="original">{t('keyboard.keepOriginal')}</option>
          {MODIFIERS.map((modifier) => (
            <option key={modifier} value={modifier}>
              {modifierLabel(modifier)}
            </option>
          ))}
          <option value="hyper">Hyper (⌃⌥⇧⌘)</option>
        </select>
      </label>
      <label className="add-form__field add-form__field--wide">
        <span>{t('keyboard.tapAction')}</span>
        <select
          className="input"
          value={selectedAction}
          onChange={(event) => {
            const tap = actions.find((action) => actionKey(action) === event.target.value);
            if (tap) patch({ tap });
          }}
        >
          {actions.map((action) => (
            <option
              key={actionKey(action)}
              value={actionKey(action)}
              disabled={action.type === 'toggleMenuBar' && !menuBarSupported}
            >
              {action.type === 'sendKeystroke'
                ? t('keyboard.sendKeystroke')
                : safeDisplayText(actionLabel(action, t))}
            </option>
          ))}
        </select>
      </label>
      {draft.tap.type === 'sendKeystroke' && (
        <div className="add-form__field add-form__field--wide">
          <label className="add-form__field">
            <span>{t('keyboard.keystroke')}</span>
            <input
              className="input"
              value={draft.tap.value}
              onChange={(event) =>
                patch({ tap: { type: 'sendKeystroke', value: event.target.value } })
              }
              aria-describedby={hintId}
            />
          </label>
          <ShortcutRecorder
            value={safeDisplayText(draft.tap.value)}
            onCapture={(value) => patch({ tap: { type: 'sendKeystroke', value } })}
            ariaLabel={t('keyboard.recordKeystroke')}
            purpose="keystroke"
          />
          <p id={hintId} className="hint">
            {t('keyboard.keystrokeHint')}
          </p>
        </div>
      )}
      <label className="modifier-editor__enabled">
        <input
          type="checkbox"
          checked={draft.enabled}
          onChange={(event) => patch({ enabled: event.target.checked })}
        />
        {t('keyboard.ruleEnabled')}
      </label>
    </fieldset>
  );
}
