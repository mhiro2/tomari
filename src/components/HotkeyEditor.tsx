import { useState } from 'react';

import * as api from '../lib/api';
import { formatCmdError } from '../lib/errors';
import { actionLabel } from '../lib/format';
import { useT } from '../lib/i18n';
import type { AppAction, Hotkey } from '../lib/types';
import { ShortcutRecorder } from './ShortcutRecorder';
import { EntityRow, Toggle } from './ui';

export interface HotkeyActionOption {
  key: string;
  label: string;
  action: AppAction;
}

export function HotkeyRow({
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
            className={`btn btn--ghost ${confirming ? 'btn--warn' : ''}`}
            onClick={handleDeleteClick}
            onBlur={() => setConfirming(false)}
            onKeyDown={(event) => {
              if (event.key === 'Escape' && confirming) {
                event.stopPropagation();
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

export function AddHotkeyForm({
  options,
  onAdded,
  onError,
}: {
  options: HotkeyActionOption[];
  onAdded: (hotkey: Hotkey) => void;
  onError: (message: string) => void;
}) {
  const t = useT();
  const [label, setLabel] = useState('');
  const [accelerator, setAccelerator] = useState('');
  const [actionKey, setActionKey] = useState(options[0]?.key ?? '');
  const [busy, setBusy] = useState(false);
  const canSubmit = label.trim() !== '' && accelerator !== '' && actionKey !== '' && !busy;

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    if (!canSubmit) return;
    const action = options.find((option) => option.key === actionKey)?.action;
    if (!action) return;
    setBusy(true);
    try {
      const hotkey: Hotkey = {
        id: `hk-${crypto.randomUUID()}`,
        label: label.trim(),
        accelerator,
        action,
        enabled: true,
      };
      await api.saveHotkey(hotkey);
      onAdded(hotkey);
      setLabel('');
      setAccelerator('');
    } catch (error) {
      onError(formatCmdError(error, t));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form className="add-form" onSubmit={(event) => void submit(event)}>
      <input
        className="input"
        placeholder={t('common.label')}
        value={label}
        onChange={(event) => setLabel(event.target.value)}
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
        onChange={(event) => setActionKey(event.target.value)}
        aria-label={t('keyboard.actionAria')}
      >
        {options.map((option) => (
          <option key={option.key} value={option.key}>
            {option.label}
          </option>
        ))}
      </select>
      <button type="submit" className="btn btn--primary" disabled={!canSubmit}>
        {t('common.add')}
      </button>
    </form>
  );
}
