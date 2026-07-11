// Turns a DOM keyboard event into an accelerator string ("Ctrl+Alt+Shift+Cmd"
// modifier order, matching the backend's canonical form). Kept free of
// React/Tauri so it is trivially unit-testable.

/** The subset of KeyboardEvent the capture logic needs. */
export interface RecorderKeyEvent {
  code: string;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}

export type CaptureResult =
  /** A complete chord was typed. */
  | { status: 'captured'; accelerator: string }
  /** A bare key that would shadow normal typing if registered globally. */
  | { status: 'needModifier' }
  /** A modifier-only press (nothing to capture yet). */
  | { status: 'ignored' }
  /** A physical key Tomari cannot register as a shortcut. */
  | { status: 'unsupported' };

// KeyboardEvent.code → backend key token, for keys whose names differ.
const CODE_KEYS: Record<string, string> = {
  ArrowLeft: 'Left',
  ArrowRight: 'Right',
  ArrowUp: 'Up',
  ArrowDown: 'Down',
  Space: 'Space',
  Tab: 'Tab',
  Enter: 'Enter',
  NumpadEnter: 'Enter',
  Escape: 'Escape',
  Backspace: 'Backspace',
  Delete: 'Delete',
  Home: 'Home',
  End: 'End',
  PageUp: 'PageUp',
  PageDown: 'PageDown',
  Minus: 'Minus',
  Equal: 'Equal',
  Comma: 'Comma',
  Period: 'Period',
  Slash: 'Slash',
  Semicolon: 'Semicolon',
  Quote: 'Quote',
  BracketLeft: 'BracketLeft',
  BracketRight: 'BracketRight',
  Backslash: 'Backslash',
  Backquote: 'Backquote',
};

// Codes that are themselves modifier keys — a lone press of these is a normal
// part of recording (the user is still building a chord), never an
// unsupported key.
const MODIFIER_CODES = new Set([
  'MetaLeft',
  'MetaRight',
  'ControlLeft',
  'ControlRight',
  'AltLeft',
  'AltRight',
  'ShiftLeft',
  'ShiftRight',
  'CapsLock',
  'Fn',
]);

/**
 * The accelerator key token for a physical key, or `null` for modifiers and
 * keys with no backend mapping. Uses `code` (not `key`) so the chord is IME-
 * and layout-independent.
 */
function keyToken(code: string): string | null {
  if (/^Key[A-Z]$/u.test(code)) return code.slice(3);
  if (/^Digit[0-9]$/u.test(code)) return code.slice(5);
  if (/^F(?:[1-9]|1[0-9]|2[0-4])$/u.test(code)) return code;
  return CODE_KEYS[code] ?? null;
}

/**
 * Interpret a keydown during recording. Function keys may stand alone; any
 * other key needs Ctrl, Alt or Cmd (Shift alone would swallow plain typing).
 */
export function captureAccelerator(event: RecorderKeyEvent): CaptureResult {
  const key = keyToken(event.code);
  if (key === null) {
    // A held modifier alone is just the chord still being built; any other
    // physical key with no backend mapping is genuinely unsupported.
    return MODIFIER_CODES.has(event.code) ? { status: 'ignored' } : { status: 'unsupported' };
  }

  const isFunctionKey = /^F\d+$/u.test(key);
  if (!event.metaKey && !event.ctrlKey && !event.altKey && !isFunctionKey) {
    return { status: 'needModifier' };
  }

  const parts: string[] = [];
  if (event.ctrlKey) parts.push('Ctrl');
  if (event.altKey) parts.push('Alt');
  if (event.shiftKey) parts.push('Shift');
  if (event.metaKey) parts.push('Cmd');
  parts.push(key);
  return { status: 'captured', accelerator: parts.join('+') };
}

/** Modifier glyphs currently held, for live feedback while recording. */
export function heldModifierGlyphs(event: RecorderKeyEvent): string[] {
  const glyphs: string[] = [];
  if (event.ctrlKey) glyphs.push('⌃');
  if (event.altKey) glyphs.push('⌥');
  if (event.shiftKey) glyphs.push('⇧');
  if (event.metaKey) glyphs.push('⌘');
  return glyphs;
}
