// Pure presentation helpers, kept free of React/Tauri so they are trivially
// unit-testable. Anything language-dependent takes the current Translator.

import type { Translator } from './i18n';
import type {
  AppAction,
  DisplayDirection,
  ImeMode,
  KeySide,
  ModifierKey,
  WindowPreset,
} from './types';

const MODIFIER_GLYPHS: Record<ModifierKey, string> = {
  capsLock: '⇪',
  control: '⌃',
  option: '⌥',
  command: '⌘',
  shift: '⇧',
  function: 'fn',
};

// Modifier key names are proper nouns, identical in both UI languages.
const MODIFIER_LABELS: Record<ModifierKey, string> = {
  capsLock: 'Caps Lock',
  control: 'Control',
  option: 'Option',
  command: 'Command',
  shift: 'Shift',
  function: 'Fn',
};

export function presetLabel(preset: WindowPreset, t: Translator): string {
  return t(`preset.${preset}`);
}

export function modifierGlyph(modifier: ModifierKey): string {
  return MODIFIER_GLYPHS[modifier];
}

export function modifierLabel(modifier: ModifierKey): string {
  return MODIFIER_LABELS[modifier];
}

export function sideLabel(side: KeySide, t: Translator): string {
  return t(`side.${side}`);
}

export function imeLabel(mode: ImeMode): string {
  return mode === 'alphanumeric' ? '英数' : 'かな';
}

export function displayDirectionLabel(direction: DisplayDirection, t: Translator): string {
  return t(`direction.${direction}`);
}

/** Human-readable one-line label for any action. */
export function actionLabel(action: AppAction, t: Translator): string {
  switch (action.type) {
    case 'togglePanel':
      return t('action.togglePanel');
    case 'snapWindow':
      return t('action.snap', { target: presetLabel(action.value, t) });
    case 'moveWindowToDisplay':
      return t('action.moveToDisplay', { display: displayDirectionLabel(action.value, t) });
    case 'undoWindow':
      return t('action.undoWindow');
    case 'launchApp':
      return t(action.value.quickPeek ? 'action.quickPeek' : 'action.launch', {
        app: action.value.bundleId,
      });
    case 'switchIme':
      return t('action.ime', { mode: imeLabel(action.value) });
    case 'sendKeystroke':
      return t('action.sendKeystroke', { keys: action.value });
    case 'toggleKeepAwake':
      return t('action.toggleKeepAwake');
    case 'noOp':
      return t('action.noOp');
  }
}

/** Render a side + modifier as e.g. "Left ⌘" or just "⇪". */
export function modifierWithSide(modifier: ModifierKey, side: KeySide, t: Translator): string {
  const glyph = modifierGlyph(modifier);
  return side === 'either' ? glyph : `${sideLabel(side, t)} ${glyph}`;
}
