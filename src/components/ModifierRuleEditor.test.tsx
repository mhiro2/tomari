import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { ModifierRule } from '../lib/types';
import { ModifierRuleEditor } from './ModifierRuleEditor';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
const { invoke } = await import('@tauri-apps/api/core');
const mockInvoke = vi.mocked(invoke);

const RULE: ModifierRule = {
  id: 'caps-rule',
  label: 'Caps Lock',
  modifier: 'capsLock',
  side: 'either',
  remapTo: 'control',
  hyper: false,
  tap: { type: 'noOp' },
  enabled: false,
};

function renderEditor(rule: ModifierRule | null = null, repairIdentity = false) {
  const onSaved = vi.fn();
  const onDeleted = vi.fn();
  const onClose = vi.fn();
  render(
    <ModifierRuleEditor
      rule={rule}
      repairIdentity={repairIdentity}
      menuBarSupported={false}
      onSaved={onSaved}
      onDeleted={onDeleted}
      onClose={onClose}
    />,
  );
  return { onSaved, onDeleted, onClose };
}

function change(label: string, value: string) {
  fireEvent.change(screen.getByLabelText(label), { target: { value } });
}

describe('ModifierRuleEditor', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation((command, args) => {
      if (command === 'save_modifier_rule')
        return Promise.resolve({ rule: (args as { rule: ModifierRule }).rule, applyWarnings: [] });
      if (command === 'delete_modifier_rule') return Promise.resolve({ applyWarnings: [] });
      if (command === 'validate_accelerator')
        return Promise.resolve({
          valid: true,
          normalized: (args as { accelerator: string }).accelerator,
          error: null,
        });
      return Promise.resolve(null);
    });
  });

  it('creates a sided Hyper rule with an IME tap without a conflicting remap', async () => {
    const { onSaved } = renderEditor();
    expect(screen.getByLabelText('Modifier rule label')).toHaveFocus();
    change('Modifier rule label', 'Hyper and Japanese');
    change('Physical key', 'control');
    change('Key side', 'right');
    change('While held', 'hyper');
    change('Tap action', 'switchIme:kana');
    fireEvent.click(screen.getByRole('button', { name: 'Save Rule' }));
    await waitFor(() => expect(onSaved).toHaveBeenCalled());
    expect(mockInvoke).toHaveBeenCalledWith('save_modifier_rule', {
      originalId: null,
      rule: expect.objectContaining({
        label: 'Hyper and Japanese',
        modifier: 'control',
        side: 'right',
        hyper: true,
        remapTo: null,
        tap: { type: 'switchIme', value: 'kana' },
        enabled: true,
      }),
    });
  });

  it('preserves raw identity, label, and disabled state while editing another field', async () => {
    const raw = { ...RULE, id: ' caps-rule ', label: '  A\u0007 label  ' };
    renderEditor(raw);
    expect(screen.getByLabelText('Modifier rule label')).toHaveValue(raw.label);
    change('While held', 'option');
    fireEvent.click(screen.getByRole('button', { name: 'Save Rule' }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('save_modifier_rule', {
        originalId: raw.id,
        rule: { ...raw, remapTo: 'option' },
      }),
    );
  });

  it('repairs an empty identity without replacing or deleting another row first', async () => {
    renderEditor({ ...RULE, id: '' }, true);
    fireEvent.click(screen.getByRole('button', { name: 'Save Rule' }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('save_modifier_rule', {
        originalId: '',
        rule: expect.objectContaining({ id: expect.stringMatching(/^rule-/u), enabled: false }),
      }),
    );
    expect(mockInvoke.mock.calls.some(([command]) => command === 'delete_modifier_rule')).toBe(
      false,
    );
  });

  it('requires an explicit hold choice to repair contradictory Hyper and remap settings', () => {
    renderEditor({ ...RULE, hyper: true });
    expect(screen.getByLabelText('While held')).toHaveValue('conflict');
    expect(screen.getByRole('button', { name: 'Save Rule' })).toBeDisabled();
    change('While held', 'hyper');
    expect(screen.getByRole('button', { name: 'Save Rule' })).not.toBeDisabled();
  });

  it('explains reserved Command sides and permits the existing Either rule semantics', () => {
    renderEditor(RULE);
    change('Physical key', 'command');
    change('Key side', 'left');
    expect(screen.getByRole('alert')).toHaveTextContent('reserved for the input switch');
    expect(screen.getByRole('button', { name: 'Save Rule' })).toBeDisabled();
    change('Key side', 'either');
    expect(screen.getByRole('button', { name: 'Save Rule' })).not.toBeDisabled();
  });

  it('requires repairing an impossible Caps Lock side instead of silently changing it', () => {
    renderEditor({ ...RULE, side: 'left' });
    expect(screen.getByLabelText('Key side')).toHaveValue('left');
    expect(screen.getByRole('alert')).toHaveTextContent('have no left or right side');
    expect(screen.getByRole('button', { name: 'Save Rule' })).toBeDisabled();
    change('Key side', 'either');
    expect(screen.getByRole('button', { name: 'Save Rule' })).not.toBeDisabled();
    change('Physical key', 'function');
    expect(
      within(screen.getByLabelText('Key side')).getByRole('option', { name: 'Left' }),
    ).toBeDisabled();
  });

  it('records a plain Escape for a tap action and restores global shortcuts', async () => {
    renderEditor(RULE);
    change('Tap action', 'sendKeystroke');
    const recorder = screen.getByRole('button', { name: 'Record keystroke to send' });
    fireEvent.click(recorder);
    await screen.findByText('Press the key to send…');
    fireEvent.keyDown(recorder, { code: 'Escape' });
    await waitFor(() => expect(screen.getByLabelText('Keystroke')).toHaveValue('Escape'));
    expect(mockInvoke).toHaveBeenCalledWith('validate_accelerator', {
      accelerator: 'Escape',
      purpose: 'keystroke',
    });
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('set_hotkeys_suspended', { suspended: false }),
    );
    fireEvent.click(screen.getByRole('button', { name: 'Save Rule' }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('save_modifier_rule', {
        originalId: RULE.id,
        rule: { ...RULE, tap: { type: 'sendKeystroke', value: 'Escape' } },
      }),
    );
  });

  it('releases recording when the tap action changes away from key sending', async () => {
    renderEditor(RULE);
    change('Tap action', 'sendKeystroke');
    fireEvent.click(screen.getByRole('button', { name: 'Record keystroke to send' }));
    await screen.findByText('Press the key to send…');
    change('Tap action', 'noOp');
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('set_hotkeys_suspended', { suspended: false }),
    );
  });

  it('keeps the draft and announces a rejected save so it can be corrected', async () => {
    mockInvoke.mockRejectedValue({ code: 'other', message: 'That modifier slot is already used.' });
    renderEditor(RULE);
    change('Modifier rule label', 'My rule');
    fireEvent.click(screen.getByRole('button', { name: 'Save Rule' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('already used');
    expect(screen.getByLabelText('Modifier rule label')).toHaveValue('My rule');
    expect(screen.getByRole('button', { name: 'Save Rule' })).not.toBeDisabled();
  });

  it('locks mutations and dismissal while a save is pending', async () => {
    let resolveSave: ((value: unknown) => void) | undefined;
    mockInvoke.mockImplementation(
      () =>
        new Promise((resolve) => {
          resolveSave = resolve;
        }),
    );
    const { onClose, onSaved } = renderEditor(RULE);
    const save = screen.getByRole('button', { name: 'Save Rule' });
    fireEvent.click(save);
    fireEvent.click(save);
    expect(mockInvoke).toHaveBeenCalledTimes(1);
    expect(screen.getByLabelText('Modifier rule label')).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Cancel' })).toBeDisabled();
    fireEvent(
      screen.getByRole('dialog'),
      new Event('cancel', { bubbles: false, cancelable: true }),
    );
    expect(onClose).not.toHaveBeenCalled();
    resolveSave?.({ rule: RULE, applyWarnings: [] });
    await waitFor(() => expect(onSaved).toHaveBeenCalled());
  });

  it('cancels without writing or deleting the draft', () => {
    const { onClose } = renderEditor(RULE);
    change('Modifier rule label', 'Discard this');
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(onClose).toHaveBeenCalledOnce();
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it('confirms deletion, allows Escape to cancel confirmation, and deletes the raw row identity', async () => {
    const { onDeleted, onClose } = renderEditor({ ...RULE, id: ' raw id ' });
    fireEvent.click(screen.getByRole('button', { name: 'Delete modifier rule Caps Lock' }));
    const confirmation = screen.getByRole('button', { name: 'Delete Caps Lock?' });
    expect(confirmation).toHaveFocus();
    expect(mockInvoke).not.toHaveBeenCalled();
    fireEvent.keyDown(confirmation, { key: 'Escape' });
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Delete modifier rule Caps Lock' }));
    fireEvent.click(screen.getByRole('button', { name: 'Delete Caps Lock?' }));
    await waitFor(() => expect(onDeleted).toHaveBeenCalledWith({ applyWarnings: [] }, ' raw id '));
    expect(mockInvoke).toHaveBeenCalledWith('delete_modifier_rule', { id: ' raw id ' });
  });

  it('preserves exact-snap behavior and does not offer unsupported menu-bar actions', async () => {
    const exactRule: ModifierRule = {
      ...RULE,
      tap: { type: 'snapWindowExact', value: 'leftHalf' },
    };
    renderEditor(exactRule);
    expect(screen.getByLabelText('Tap action')).toHaveValue('snapWindowExact:leftHalf');
    expect(
      screen.queryByRole('option', { name: 'Show/Hide Menu Bar Icons' }),
    ).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Save Rule' }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('save_modifier_rule', {
        originalId: RULE.id,
        rule: exactRule,
      }),
    );
  });
});
