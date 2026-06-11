// Thin, typed wrappers over the Tauri command bridge. The argument keys here
// must match the `#[tauri::command]` parameter names on the Rust side.

import { invoke } from '@tauri-apps/api/core';

import type {
  AcceleratorCheck,
  AppAction,
  AppSettings,
  DisplayDirection,
  ExportOutcome,
  Hotkey,
  ImportOutcome,
  KeepAwakeStatus,
  ModifierRule,
  UpdateInfo,
  WindowPreset,
} from './types';

export const getSettings = (): Promise<AppSettings> => invoke('get_settings');

export const saveSettings = (settings: AppSettings): Promise<void> =>
  invoke('save_settings', { settings });

export const listHotkeys = (): Promise<Hotkey[]> => invoke('list_hotkeys');

export const saveHotkey = (hotkey: Hotkey): Promise<void> => invoke('save_hotkey', { hotkey });

export const deleteHotkey = (id: string): Promise<void> => invoke('delete_hotkey', { id });

export const listModifierRules = (): Promise<ModifierRule[]> => invoke('list_modifier_rules');

export const saveModifierRule = (rule: ModifierRule): Promise<void> =>
  invoke('save_modifier_rule', { rule });

export const deleteModifierRule = (id: string): Promise<void> =>
  invoke('delete_modifier_rule', { id });

export const listWindowPresets = (): Promise<WindowPreset[]> => invoke('list_window_presets');

// Resolves to the preset actually applied (repeated half-snaps cycle
// 1/2 → 1/3 → 2/3), or null when window management is disabled.
export const snapWindow = (preset: WindowPreset): Promise<WindowPreset | null> =>
  invoke('snap_window', { preset });

export const moveWindowToDisplay = (direction: DisplayDirection): Promise<void> =>
  invoke('move_window_to_display', { direction });

export const undoWindow = (): Promise<void> => invoke('undo_window');

export const accessibilityStatus = (): Promise<boolean> => invoke('accessibility_status');

export const requestAccessibility = (): Promise<boolean> => invoke('request_accessibility');

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

// Resolves to the available update, or null when already on the latest version.
export const checkForUpdate = (): Promise<UpdateInfo | null> => invoke('check_for_update');

// Downloads and applies the update found by checkForUpdate, then relaunches —
// on success this promise never settles because the app restarts.
export const installUpdate = (): Promise<void> => invoke('install_update');

// Opens a native save dialog and writes the whole configuration as JSON.
// Resolves to the outcome (saved with the path, or cancelled).
export const exportConfig = (): Promise<ExportOutcome> => invoke('export_config');

// Opens a native open dialog, validates the chosen file, and — only if it is
// wholly valid — replaces the current configuration with it (backing the
// current one up first). Resolves to the outcome (applied, rejected, cancelled).
export const importConfig = (): Promise<ImportOutcome> => invoke('import_config');
