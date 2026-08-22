// Thin, typed wrappers over the Tauri command bridge. The argument keys here
// must match the `#[tauri::command]` parameter names on the Rust side.

import { invoke } from '@tauri-apps/api/core';

import type {
  AcceleratorCheck,
  AppAction,
  AppSettings,
  DisplayDirection,
  Hotkey,
  KeepAwakeStatus,
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

export const runAction = (action: AppAction): Promise<void> => invoke('run_action', { action });

// Current sleep-prevention status, for the panel to render on open.
export const getKeepAwake = (): Promise<KeepAwakeStatus> => invoke('get_keep_awake');

// Turn sleep prevention on or off. Resolves to the resulting status; lidClose
// may flip shortly after (the lid-close veto prompts for admin in the
// background), signalled by the "tomari:keep-awake-changed" event.
export const setKeepAwake = (enabled: boolean): Promise<KeepAwakeStatus> =>
  invoke('set_keep_awake', { enabled });

// Whether menu bar tidying is on and, if so, whether it is collapsed.
export const getMenuBar = (): Promise<MenuBarStatus> => invoke('get_menu_bar');

// Expand or collapse the tidied menu bar icons. Resolves to the resulting
// status, which reports the feature still off if it was never switched on.
export const setMenuBarCollapsed = (collapsed: boolean): Promise<MenuBarStatus> =>
  invoke('set_menu_bar_collapsed', { collapsed });

// Resolves to the available update, or null when already on the latest version.
export const checkForUpdate = (): Promise<UpdateInfo | null> => invoke('check_for_update');

// Downloads and applies the update found by checkForUpdate, then relaunches —
// on success this promise never settles because the app restarts.
export const installUpdate = (): Promise<void> => invoke('install_update');
