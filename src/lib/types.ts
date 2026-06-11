// TypeScript mirror of the `tomari-core` domain types. These match the
// camelCase JSON the Rust backend produces and accepts.

export type WindowPreset =
  | 'leftHalf'
  | 'rightHalf'
  | 'topHalf'
  | 'bottomHalf'
  | 'topLeftQuarter'
  | 'topRightQuarter'
  | 'bottomLeftQuarter'
  | 'bottomRightQuarter'
  | 'leftThird'
  | 'centerThird'
  | 'rightThird'
  | 'leftTwoThirds'
  | 'rightTwoThirds'
  | 'center'
  | 'maximize';

export type DisplayDirection = 'next' | 'prev';

export type ModifierKey = 'capsLock' | 'control' | 'option' | 'command' | 'shift' | 'function';

export type KeySide = 'left' | 'right' | 'either';

export type ImeMode = 'alphanumeric' | 'kana';

export type Theme = 'system' | 'light' | 'dark';

export type Language = 'system' | 'en' | 'ja';

export interface LaunchTarget {
  bundleId: string;
  quickPeek: boolean;
}

// Adjacently-tagged enum: serde `#[serde(tag = "type", content = "value")]`.
export type AppAction =
  | { type: 'togglePanel' }
  | { type: 'snapWindow'; value: WindowPreset }
  | { type: 'moveWindowToDisplay'; value: DisplayDirection }
  | { type: 'undoWindow' }
  | { type: 'launchApp'; value: LaunchTarget }
  | { type: 'switchIme'; value: ImeMode }
  | { type: 'sendKeystroke'; value: string }
  | { type: 'toggleKeepAwake' }
  | { type: 'noOp' };

export interface Hotkey {
  id: string;
  label: string;
  accelerator: string;
  action: AppAction;
  enabled: boolean;
}

export interface ModifierRule {
  id: string;
  label: string;
  modifier: ModifierKey;
  side: KeySide;
  remapTo?: ModifierKey | null;
  hyper: boolean;
  tap: AppAction;
  enabled: boolean;
}

export interface AppSettings {
  launchAtLogin: boolean;
  theme: Theme;
  language: Language;
  keyboardEnabled: boolean;
  windowManagementEnabled: boolean;
  externalWindowActionsEnabled: boolean;
  holdThresholdMs: number;
  showInMenuBar: boolean;
  dragToSnapEnabled: boolean;
}

// State of the lid-close veto (pmset disablesleep): off, awaiting admin auth,
// engaged, or unavailable because authorization was declined.
export type LidCloseState = 'off' | 'pending' | 'engaged' | 'unavailable';

// Runtime sleep-prevention state (not part of AppSettings — it never persists).
export interface KeepAwakeStatus {
  // Sleep prevention is on.
  active: boolean;
  // Lid-close veto state — when "engaged", work continues with the lid shut.
  lidClose: LidCloseState;
}

export interface AcceleratorCheck {
  valid: boolean;
  normalized: string | null;
  error: string | null;
}

// Error shape a #[tauri::command] rejects with. `code` classifies the frequent,
// localizable failures; `message` is the developer-facing English fallback for
// everything else (`code: "other"`).
export type CmdErrorCode = 'permissionRequired' | 'noFocusedWindow' | 'shortcutConflict' | 'other';

export interface CmdError {
  code: CmdErrorCode;
  message: string;
}

// A newer version reported by the update endpoint.
export interface UpdateInfo {
  version: string;
  notes: string | null;
}

// Summary of a configuration import that was applied.
export interface ImportReport {
  hotkeys: number;
  modifierRules: number;
  warnings: string[];
  registrationFailures: string[];
  backupPath: string;
}

// Outcome of an import. `rejected` carries every validation problem found, so
// the file can be fixed in one pass; nothing was changed in that case.
export type ImportOutcome =
  | { status: 'cancelled' }
  | { status: 'rejected'; errors: string[] }
  | { status: 'applied'; report: ImportReport };

// Outcome of an export. `omitted` counts stored rows that could not be read and
// were left out of the file.
export type ExportOutcome =
  | { status: 'cancelled' }
  | { status: 'saved'; path: string; omitted: number };
