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

export type Language = 'system' | 'en' | 'ja';

// Adjacently-tagged enum: serde `#[serde(tag = "type", content = "value")]`.
// Mirror of the Rust `AppAction` (crates/tomari-core/src/domain/action.rs); the
// contract test there pins each variant's `type` tag so this list stays in sync.
export type AppAction =
  | { type: 'togglePanel' }
  | { type: 'snapWindow'; value: WindowPreset }
  // Like snapWindow but never cycles — emitted by the tomari:// URL scheme.
  | { type: 'snapWindowExact'; value: WindowPreset }
  | { type: 'moveWindowToDisplay'; value: DisplayDirection }
  | { type: 'undoWindow' }
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
  language: Language;
  keyboardEnabled: boolean;
  windowManagementEnabled: boolean;
  externalWindowActionsEnabled: boolean;
  commandImeSwitchEnabled: boolean;
  showInMenuBar: boolean;
  dragToSnapEnabled: boolean;
  dragToMoveEnabled: boolean;
}

// State of the lid-close veto (pmset disablesleep): off, awaiting admin auth, or
// engaged. ('unavailable' mirrors the backend enum but is no longer surfaced — a
// declined veto turns keep-awake off entirely rather than reporting it.)
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

// Result of saveSettings: the settings always persist (a write failure rejects
// the command instead), but a side effect — registering the login item, showing
// or hiding the menu bar icon — may still fail to apply. Each code in
// `applyWarnings` names one that did, so the UI can warn that the stored
// preference and the live system state disagree until retried. Empty on a fully
// applied save.
export interface SaveSettingsOutcome {
  applyWarnings: string[];
}
