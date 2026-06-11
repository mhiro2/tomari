import { describe, expect, it } from 'vitest';

import { captureAccelerator, heldModifierGlyphs, type RecorderKeyEvent } from './recorder';

function event(code: string, mods: Partial<RecorderKeyEvent> = {}): RecorderKeyEvent {
  return { code, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...mods };
}

describe('captureAccelerator', () => {
  it('captures letters and digits with modifiers in canonical order', () => {
    expect(captureAccelerator(event('KeyL', { metaKey: true, shiftKey: true }))).toEqual({
      status: 'captured',
      accelerator: 'Shift+Cmd+L',
    });
    expect(captureAccelerator(event('Digit3', { ctrlKey: true, altKey: true }))).toEqual({
      status: 'captured',
      accelerator: 'Ctrl+Alt+3',
    });
  });

  it('maps arrow and named keys to backend tokens', () => {
    expect(captureAccelerator(event('ArrowLeft', { metaKey: true }))).toEqual({
      status: 'captured',
      accelerator: 'Cmd+Left',
    });
    expect(captureAccelerator(event('NumpadEnter', { ctrlKey: true }))).toEqual({
      status: 'captured',
      accelerator: 'Ctrl+Enter',
    });
  });

  it('allows function keys without any modifier', () => {
    expect(captureAccelerator(event('F5'))).toEqual({ status: 'captured', accelerator: 'F5' });
    expect(captureAccelerator(event('F24', { shiftKey: true }))).toEqual({
      status: 'captured',
      accelerator: 'Shift+F24',
    });
  });

  it('requires Ctrl/Alt/Cmd for ordinary keys', () => {
    expect(captureAccelerator(event('KeyA'))).toEqual({ status: 'needModifier' });
    expect(captureAccelerator(event('KeyA', { shiftKey: true }))).toEqual({
      status: 'needModifier',
    });
  });

  it('ignores modifier-only and unsupported keys', () => {
    expect(captureAccelerator(event('MetaLeft', { metaKey: true }))).toEqual({
      status: 'ignored',
    });
    expect(captureAccelerator(event('CapsLock'))).toEqual({ status: 'ignored' });
    expect(captureAccelerator(event('F25'))).toEqual({ status: 'ignored' });
    expect(captureAccelerator(event('IntlYen', { metaKey: true }))).toEqual({
      status: 'ignored',
    });
  });
});

describe('heldModifierGlyphs', () => {
  it('lists held modifiers in canonical order', () => {
    expect(
      heldModifierGlyphs(event('MetaLeft', { metaKey: true, ctrlKey: true, shiftKey: true })),
    ).toEqual(['⌃', '⇧', '⌘']);
    expect(heldModifierGlyphs(event('KeyA'))).toEqual([]);
  });
});
