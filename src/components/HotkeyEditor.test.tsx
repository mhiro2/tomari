import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { Hotkey } from '../lib/types';
import { AddHotkeyForm, HotkeyRow, type HotkeyActionOption } from './HotkeyEditor';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
const { invoke } = await import('@tauri-apps/api/core');
const mockInvoke = vi.mocked(invoke);

const OPTIONS: HotkeyActionOption[] = [
  { key: 'togglePanel', label: 'Toggle Tomari', action: { type: 'togglePanel' } },
];

describe('AddHotkeyForm', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
  });

  it('adds the canonical hotkey returned by the backend', async () => {
    const onAdded = vi.fn();
    const onError = vi.fn();
    mockInvoke.mockImplementation((cmd: string, args?: unknown) => {
      switch (cmd) {
        case 'set_hotkeys_suspended':
          return Promise.resolve(undefined);
        case 'validate_accelerator':
          return Promise.resolve({ valid: true, normalized: 'Cmd+Plus', error: null });
        case 'save_hotkey': {
          const submitted = (args as { hotkey: Hotkey }).hotkey;
          return Promise.resolve({
            ...submitted,
            id: 'canonical-id',
            accelerator: 'Shift+Cmd+Equal',
          });
        }
        default:
          return Promise.resolve(null);
      }
    });

    render(<AddHotkeyForm options={OPTIONS} onAdded={onAdded} onError={onError} />);
    fireEvent.change(screen.getByLabelText('Shortcut label'), {
      target: { value: 'Zoom in' },
    });
    const recorder = screen.getByRole('button', { name: 'Record Shortcut' });
    fireEvent.click(recorder);
    await screen.findByText('Type shortcut…');
    fireEvent.keyDown(recorder, { code: 'Equal', metaKey: true, shiftKey: true });
    await waitFor(() => expect(screen.getByRole('button', { name: 'Add' })).not.toBeDisabled());
    fireEvent.click(screen.getByRole('button', { name: 'Add' }));

    await waitFor(() =>
      expect(onAdded).toHaveBeenCalledWith(
        expect.objectContaining({ id: 'canonical-id', accelerator: 'Shift+Cmd+Equal' }),
      ),
    );
    expect(onError).not.toHaveBeenCalled();
  });
});

describe('HotkeyRow', () => {
  it('sanitizes persisted labels, action values, and accelerators for display', () => {
    const hotkey: Hotkey = {
      id: 'unsafe-display',
      label: `Safe\u0000\u202e${'界'.repeat(120)}`,
      accelerator: 'Cmd+\u202eK\u0000',
      action: { type: 'sendKeystroke', value: 'K\u202e\u0000' },
      enabled: false,
    };
    const { container } = render(
      <HotkeyRow
        hotkey={hotkey}
        saving={false}
        onAccelerator={vi.fn()}
        onToggle={vi.fn()}
        onDelete={vi.fn()}
      />,
    );

    expect(container.textContent).not.toMatch(/[\p{Cc}\p{Cf}]/u);
    expect(screen.getByText(/^Safe 界+…$/u)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Change shortcut for Send: K' })).toHaveTextContent(
      '⌘K',
    );
  });
});
