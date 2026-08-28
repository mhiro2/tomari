// Thin, typed wrappers over the Tauri command bridge. The argument keys here
// must match the `#[tauri::command]` parameter names on the Rust side.

import { invoke } from '@tauri-apps/api/core';

import type {
  AcceleratorCheck,
  AppSettings,
  DisplayDirection,
  Hotkey,
  KeepAwakeOptions,
  LiveApplyWarnings,
  KeepAwakeStatus,
  MenuBarInventory,
  MenuBarItemZone,
  MenuBarMoveResult,
  MenuBarStatus,
  ModifierRule,
  SaveSettingsOutcome,
  SetupStatus,
  UpdateInfo,
  PlacementContext,
  PlacementEditResult,
  PlacementSlot,
  WindowTarget,
  WindowHistoryStatus,
  HistoryActionResult,
  MoveRecallResult,
} from './types';

export const getSettings = (): Promise<AppSettings> => invoke('get_settings');
// The `applyWarnings` the live state warrants right now, independent of any
// save — read when the panel opens so a mismatch left over from an earlier
// session (a Caps Lock restore that failed on quit, say) is shown at once.
export const getApplyWarnings = (): Promise<LiveApplyWarnings> => invoke('get_apply_warnings');

// Resolves once the settings are persisted; `applyWarnings` lists any side
// effect that saved but could not be applied to the system.
export const saveSettings = (settings: AppSettings): Promise<SaveSettingsOutcome> =>
  invoke('save_settings', { settings });

export const listHotkeys = (): Promise<Hotkey[]> => invoke('list_hotkeys');

export const saveHotkey = (hotkey: Hotkey): Promise<void> => invoke('save_hotkey', { hotkey });

export const deleteHotkey = (id: string): Promise<void> => invoke('delete_hotkey', { id });

export const listModifierRules = (): Promise<ModifierRule[]> => invoke('list_modifier_rules');

export const saveModifierRule = (rule: ModifierRule): Promise<void> =>
  invoke('save_modifier_rule', { rule });

export const deleteModifierRule = (id: string): Promise<void> =>
  invoke('delete_modifier_rule', { id });

export const getPlacementContext = (): Promise<PlacementContext> => invoke('get_placement_context');

export const captureWindowPlacement = (
  target: WindowTarget,
  slot: PlacementSlot,
): Promise<PlacementEditResult> => invoke('capture_window_placement', { target, slot });

export const forgetWindowPlacement = (
  target: WindowTarget,
  slot: PlacementSlot,
): Promise<PlacementEditResult> => invoke('forget_window_placement', { target, slot });

export const undoWindowPlacementEdit = (): Promise<HistoryActionResult> =>
  invoke('undo_window_placement_edit');

export const recallWindowPlacement = (target: WindowTarget): Promise<PlacementSlot> =>
  invoke('recall_window_placement', { target });

export const moveWindowToDisplayAndRecall = (
  target: WindowTarget,
  direction: DisplayDirection,
): Promise<MoveRecallResult> => invoke('move_window_to_display_and_recall', { target, direction });

export const getWindowHistoryStatus = (): Promise<WindowHistoryStatus> =>
  invoke('get_window_history_status');

export const undoWindow = (): Promise<HistoryActionResult> => invoke('undo_window');

export const redoWindow = (): Promise<HistoryActionResult> => invoke('redo_window');

// Startup pull for the setup checklist: whether this is a first run, whether an
// update looks to have revoked permissions, and the current permission states.
export const setupStatus = (): Promise<SetupStatus> => invoke('setup_status');

export const accessibilityStatus = (): Promise<boolean> => invoke('accessibility_status');

export const requestAccessibility = (): Promise<boolean> => invoke('request_accessibility');

export const inputMonitoringStatus = (): Promise<boolean> => invoke('input_monitoring_status');

export const requestInputMonitoring = (): Promise<boolean> => invoke('request_input_monitoring');

export const validateAccelerator = (accelerator: string): Promise<AcceleratorCheck> =>
  invoke('validate_accelerator', { accelerator });

// Temporarily unregister (true) or re-register (false) all global shortcuts,
// so a shortcut being recorded reaches the panel instead of firing an action.
export const setHotkeysSuspended = (suspended: boolean): Promise<void> =>
  invoke('set_hotkeys_suspended', { suspended });

// Current sleep-prevention status, for the panel to render on open.
export const getKeepAwake = (): Promise<KeepAwakeStatus> => invoke('get_keep_awake');

// Turn sleep prevention on or off. The first response can be an explicit
// enabling/disabling phase; completion is signalled by the changed event.
export const setKeepAwake = (
  enabled: boolean,
  options?: KeepAwakeOptions,
): Promise<KeepAwakeStatus> => invoke('set_keep_awake', { enabled, options });

export const configureKeepAwake = (options: KeepAwakeOptions): Promise<KeepAwakeStatus> =>
  invoke('configure_keep_awake', { options });

export const cancelKeepAwakeTransition = (): Promise<KeepAwakeStatus> =>
  invoke('cancel_keep_awake_transition');

export const retryKeepAwakeTransition = (): Promise<KeepAwakeStatus> =>
  invoke('retry_keep_awake_transition');
// Leave a lid-close override found at launch in place and forget the marker
// that pointed at it (the `leftoverOverride` notice).
export const dismissKeepAwakeRecovery = (): Promise<KeepAwakeStatus> =>
  invoke('dismiss_keep_awake_recovery');

// Whether menu bar tidying is on and, if so, whether it is collapsed.
export const getMenuBar = (): Promise<MenuBarStatus> => invoke('get_menu_bar');

// Read the real hidden/visible item arrangement around Tomari's divider.
export const listMenuBarItems = (): Promise<MenuBarInventory> => invoke('list_menu_bar_items');

// Move one item across Tomari's divider and return the fresh arrangement that
// the backend verified after macOS finished reflowing the menu bar.
export const moveMenuBarItem = (
  itemId: string,
  targetZone: MenuBarItemZone,
): Promise<MenuBarMoveResult> => invoke('move_menu_bar_item', { itemId, targetZone });

// Expand or collapse the tidied menu bar icons. Resolves to the resulting
// status, which reports the feature still off if it was never switched on.
export const setMenuBarCollapsed = (collapsed: boolean): Promise<MenuBarStatus> =>
  invoke('set_menu_bar_collapsed', { collapsed });

// Resolves to the available update, or null when already on the latest version.
export const checkForUpdate = (): Promise<UpdateInfo | null> => invoke('check_for_update');

// Downloads and applies the update found by checkForUpdate, then relaunches —
// on success this promise never settles because the app restarts.
export const installUpdate = (): Promise<void> => invoke('install_update');
