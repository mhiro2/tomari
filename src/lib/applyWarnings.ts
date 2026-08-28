// The user-facing text for an `applyWarnings` code returned by a save. Shared
// by every view that renders those codes, so a code added to the backend has one
// place to gain its wording.
import type { Translator } from './i18n';

export function applyWarningText(code: string, t: Translator): string {
  switch (code) {
    case 'launchAtLogin':
      return t('settings.applyWarning.launchAtLogin');
    case 'menuBar':
      return t('settings.applyWarning.menuBar');
    case 'keyboardTap':
      return t('settings.applyWarning.keyboardTap');
    case 'globalShortcuts':
      return t('settings.applyWarning.globalShortcuts');
    case 'dragToSnapTap':
      return t('settings.applyWarning.dragToSnapTap');
    case 'dragToMoveTap':
      return t('settings.applyWarning.dragToMoveTap');
    case 'capsLockRemap':
      return t('settings.applyWarning.capsLockRemap');
    case 'commandImeRules':
      return t('settings.applyWarning.commandImeRules');
    default:
      return t('settings.applyWarning.generic');
  }
}
