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

export type PlacementSlot = 'primary' | 'secondary';

export interface NormalizedRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface WindowApplication {
  bundleId: string;
  name: string;
}

export interface WindowPlacement {
  application: WindowApplication;
  slot: PlacementSlot;
  frame: NormalizedRect;
}

export interface WindowTarget {
  bundleId: string;
  windowId: string;
}

export interface PlacementContext {
  target: WindowTarget;
  application: WindowApplication;
  currentFrame: NormalizedRect;
  placements: WindowPlacement[];
  canMoveToDisplay: boolean;
}

export interface WindowHistoryStatus {
  canUndo: boolean;
  canRedo: boolean;
}

export type HistoryActionResult = 'applied' | 'empty' | 'staleEntriesDiscarded';

export type MoveRecallResult =
  | { status: 'moved'; slot: PlacementSlot }
  | { status: 'noAdjacentDisplay' };

export interface PlacementEditResult {
  changed: boolean;
  undoable: boolean;
}

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
  | { type: 'recallWindowPlacement' }
  | { type: 'moveWindowToDisplayAndRecall'; value: DisplayDirection }
  | { type: 'undoWindow' }
  | { type: 'redoWindow' }
  | { type: 'switchIme'; value: ImeMode }
  | { type: 'sendKeystroke'; value: string }
  | { type: 'toggleKeepAwake' }
  | { type: 'toggleMenuBar' }
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
  menuBarTidyEnabled: boolean;
  menuBarAutoCollapseSecs: number;
}

// State of the lid-close veto (pmset disablesleep): off, awaiting admin auth, or
// engaged. ('unavailable' mirrors the backend enum but is no longer surfaced — a
// declined veto turns keep-awake off entirely rather than reporting it.)
export type LidCloseState = 'off' | 'pending' | 'engaged' | 'unavailable';
export type KeepAwakePhase = 'off' | 'enabling' | 'on' | 'disabling' | 'failed';
export type PowerSource = 'ac' | 'battery' | 'unknown';
export type LowBatteryAction = 'warn' | 'turnOff';
export type KeepAwakeNotice =
  | 'acRequired'
  | 'acDisconnected'
  | 'lowBattery'
  | 'timerElapsed'
  | 'authorizationDeclined'
  | 'lidCloseUnconfirmed';

export interface KeepAwakeOptions {
  durationSecs: number | null;
  endsAtMs: number | null;
  acOnly: boolean;
  lowBatteryAction: LowBatteryAction;
}

export interface LongRunningProcess {
  pid: number;
  name: string;
  elapsedSecs: number;
}

// Runtime sleep-prevention state (not part of AppSettings — it never persists).
export interface KeepAwakeStatus {
  // Sleep prevention is on.
  active: boolean;
  // Lid-close veto state — when "engaged", work continues with the lid shut.
  lidClose: LidCloseState;
  phase: KeepAwakePhase;
  options: KeepAwakeOptions;
  notice: KeepAwakeNotice | null;
  powerSource: PowerSource;
  batteryPercent: number | null;
  kernelSleepDisabled: boolean | null;
  ownsLidClose: boolean;
  longRunningProcesses: LongRunningProcess[];
  // Monotonic ordering stamp, assigned by the backend as it emits. Several
  // backend threads emit, so events can arrive out of order; the panel drops any
  // snapshot older than one it has already applied. Not for display.
  revision: number;
}

// Runtime menu-bar-tidy state. Like keep-awake this never persists: a launch
// always starts collapsed.
export interface MenuBarStatus {
  // The feature's master switch, mirroring `menuBarTidyEnabled`.
  enabled: boolean;
  // Whether the tidied icons are currently pushed off-screen.
  collapsed: boolean;
}

export type MenuBarItemZone = 'hidden' | 'visible';

// One currently running menu bar item discovered through Accessibility. IDs
// are snapshot-local because macOS does not expose a durable status-item id.
export interface MenuBarItem {
  id: string;
  name: string;
  ownerName: string | null;
  bundleId: string | null;
  zone: MenuBarItemZone;
}

export interface MenuBarInventory {
  supported: boolean;
  permissionGranted: boolean;
  dividerAvailable: boolean;
  items: MenuBarItem[];
}

export type MenuBarMoveOutcome = 'moved' | 'alreadyInZone' | 'staleItem' | 'notMovable';

export interface MenuBarMoveResult {
  outcome: MenuBarMoveOutcome;
  inventory: MenuBarInventory;
}

export interface AcceleratorCheck {
  valid: boolean;
  normalized: string | null;
  error: string | null;
}

// Payload of "tomari:permissions-changed", emitted when Accessibility or Input
// Monitoring transitions (granted in System Settings, outside the app).
export interface PermissionsChanged {
  accessibility: boolean;
  inputMonitoring: boolean;
}

// Result of the setup_status command, pulled once at startup to populate the
// permission reminder and update-specific recovery copy.
export interface SetupStatus {
  // This launch seeded the database (a true first run).
  firstRun: boolean;
  // Previously granted permissions look lost to an app update.
  updateRegrant: boolean;
  accessibility: boolean;
  inputMonitoring: boolean;
}

// Error shape a #[tauri::command] rejects with. `code` classifies the frequent,
// localizable failures; `message` is the developer-facing English fallback for
// everything else (`code: "other"`).
export type CmdErrorCode =
  | 'permissionRequired'
  | 'noFocusedWindow'
  | 'shortcutConflict'
  | 'placementNotFound'
  | 'windowTargetChanged'
  | 'windowNotResponding'
  | 'other';

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
