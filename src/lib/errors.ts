import type { Translator } from './i18n';
import type { CmdErrorCode } from './types';

/** Last-resort stringify for a value that is not a structured command error. */
export function errorText(e: unknown): string {
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  return String(e);
}

// Command-error codes the UI has a localized message for; anything else falls
// back to the (English) `message` the backend attached.
const LOCALIZED_CODES = ['permissionRequired', 'noFocusedWindow', 'shortcutConflict'] as const;
type LocalizedCode = (typeof LOCALIZED_CODES)[number];

function isLocalized(code: CmdErrorCode): code is LocalizedCode {
  return (LOCALIZED_CODES as readonly string[]).includes(code);
}

/**
 * Turn a command rejection into display text. Tauri rejects with the serialized
 * `CmdError` (`{ code, message }`): a known `code` is translated, otherwise the
 * `message` (or any non-command error) is shown verbatim.
 */
export function formatCmdError(e: unknown, t: Translator): string {
  if (e && typeof e === 'object' && 'code' in e) {
    const err = e as { code: CmdErrorCode; message?: string };
    if (isLocalized(err.code)) return t(`error.${err.code}`);
    if (typeof err.message === 'string' && err.message !== '') return err.message;
  }
  return errorText(e);
}

/**
 * The `message` of a command rejection, without translation — for failures
 * that never carry a localizable `code` (the updater, network checks), so the
 * formatter does not capture the reactive `t` where it isn't needed.
 */
export function cmdErrorMessage(e: unknown): string {
  if (e && typeof e === 'object' && 'message' in e) {
    const message = (e as { message?: unknown }).message;
    if (typeof message === 'string' && message !== '') return message;
  }
  return errorText(e);
}
