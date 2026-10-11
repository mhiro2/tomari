import { invoke } from '@tauri-apps/api/core';

import { safeDisplayText } from './format';
import type { MessageKey, Translator } from './i18n';

export interface DisplayMode {
  id: number;
  width: number;
  height: number;
  pixelWidth: number;
  pixelHeight: number;
  refreshRate: number;
  hiDpi: boolean;
}

export interface DisplayInfo {
  connectionId: number;
  id: string;
  name: string;
  builtIn: boolean;
  main: boolean;
  mirrored: boolean;
  x: number;
  y: number;
  width: number;
  height: number;
  currentMode: DisplayMode | null;
  modes: DisplayMode[];
  brightness: number | null;
  softwareDimming: number;
}

export interface DisplaySnapshot {
  displays: DisplayInfo[];
  pending: { token: number; displayId: string; remainingMs: number } | null;
  notice: 'reverted' | 'restoreFailed' | 'displayChanged' | null;
  revision: number;
}

export function displayName(display: Pick<DisplayInfo, 'builtIn' | 'name'>, t: Translator): string {
  return display.builtIn
    ? t('display.builtIn')
    : safeDisplayText(display.name) || t('display.external');
}

export function ambiguousDisplayIds(connected: readonly DisplayInfo[]): Set<string> {
  const seen = new Set<string>();
  const ambiguous = new Set<string>();
  for (const display of connected) {
    if (seen.has(display.id)) ambiguous.add(display.id);
    seen.add(display.id);
  }
  return ambiguous;
}

export interface DisplayProfile {
  id: string;
  name: string;
  displays: {
    displayId: string;
    x: number;
    y: number;
    mode: Omit<DisplayMode, 'id' | 'hiDpi'>;
  }[];
}

export const getDisplayProfiles = (): Promise<DisplayProfile[]> => invoke('get_display_profiles');
export const saveDisplayProfile = (name: string): Promise<string> =>
  invoke('save_display_profile', { name });
export const deleteDisplayProfile = (id: string): Promise<void> =>
  invoke('delete_display_profile', { id });
export const renameDisplayProfile = (id: string, name: string): Promise<void> =>
  invoke('rename_display_profile', { id, name });
export const clearDisplayProfiles = (): Promise<void> => invoke('clear_display_profiles');
export const previewDisplayProfile = (id: string): Promise<DisplaySnapshot> =>
  invoke('preview_display_profile', { id });

export type DisplayConfigurationRequest =
  | { kind: 'mode'; displayId: string; modeId: number }
  | { kind: 'main'; displayId: string }
  | { kind: 'position'; displayId: string; relativeTo: string; placement: RelativePlacement };

export const getDisplays = (): Promise<DisplaySnapshot> => invoke('get_displays');
export const openSystemDisplaySettings = async (): Promise<DisplaySnapshot> => {
  await invoke('open_system_display_settings');
  return getDisplays();
};
export const identifyDisplays = async (displayIds: string[]): Promise<DisplaySnapshot> => {
  await invoke('identify_displays', { displayIds });
  return getDisplays();
};
export const getDisplayNavigation = (): Promise<number | null> => invoke('get_display_navigation');
export const acknowledgeDisplayNavigation = (revision: number): Promise<void> =>
  invoke('acknowledge_display_navigation', { revision });
export const previewDisplayMode = (displayId: string, modeId: number): Promise<DisplaySnapshot> =>
  invoke('preview_display_configuration', { request: { kind: 'mode', displayId, modeId } });
export const previewMainDisplay = (displayId: string): Promise<DisplaySnapshot> =>
  invoke('preview_display_configuration', { request: { kind: 'main', displayId } });
export type RelativePlacement = 'left' | 'right' | 'above' | 'below';
export const previewDisplayPosition = (
  displayId: string,
  relativeTo: string,
  placement: RelativePlacement,
): Promise<DisplaySnapshot> =>
  invoke('preview_display_configuration', {
    request: { kind: 'position', displayId, relativeTo, placement },
  });
export const confirmDisplayMode = (token: number): Promise<DisplaySnapshot> =>
  invoke('confirm_display_configuration', { token });
export const revertDisplayMode = (token: number): Promise<DisplaySnapshot> =>
  invoke('revert_display_configuration', { token });
export const setDisplayBrightness = (displayId: string, value: number): Promise<DisplaySnapshot> =>
  invoke('set_display_brightness', { displayId, value });

export const setDisplayDimming = async (
  displayId: string,
  amount: number,
): Promise<DisplaySnapshot> => {
  await invoke('set_display_dimming', { displayId, amount });
  return getDisplays();
};
export const resetDisplayDimming = async (): Promise<DisplaySnapshot> => {
  await invoke('reset_display_dimming');
  return getDisplays();
};

const ERROR_KEYS: Record<string, MessageKey> = {
  unavailable: 'display.error.unavailable',
  disconnected: 'display.error.disconnected',
  modeUnavailable: 'display.error.modeUnavailable',
  mirrored: 'display.error.mirrored',
  busy: 'display.error.busy',
  previewExpired: 'display.error.previewExpired',
  invalidBrightness: 'display.error.invalidBrightness',
  invalidDimming: 'display.error.invalidDimming',
  invalidArrangement: 'display.error.invalidArrangement',
  brightnessUnavailable: 'display.error.brightnessUnavailable',
  system: 'display.error.system',
  refreshRequired: 'display.error.refreshRequired',
  invalidProfile: 'display.error.invalidProfile',
  profileUnavailable: 'display.error.profileUnavailable',
  profileStorage: 'display.error.profileStorage',
};

export function displayErrorKey(error: unknown): MessageKey {
  const code = typeof error === 'object' && error !== null && 'code' in error ? error.code : null;
  return (
    (typeof code === 'string' && Object.hasOwn(ERROR_KEYS, code) ? ERROR_KEYS[code] : undefined) ??
    'display.error.system'
  );
}

export function modeLabel(
  mode: Pick<DisplayMode, 'width' | 'height' | 'refreshRate' | 'hiDpi'>,
): string {
  const rate = mode.refreshRate > 0 ? ` · ${Number(mode.refreshRate.toFixed(2))} Hz` : '';
  return `${mode.width} × ${mode.height}${mode.hiDpi ? ' · HiDPI' : ''}${rate}`;
}

export interface ResolutionGroup {
  key: string;
  label: string;
  modes: DisplayMode[];
}

export function resolutionGroups(
  modes: DisplayMode[],
  currentModeId: number | undefined,
): ResolutionGroup[] {
  const groups = new Map<string, ResolutionGroup>();
  for (const mode of modes) {
    const key = `${mode.width}x${mode.height}:${mode.pixelWidth}x${mode.pixelHeight}`;
    let group = groups.get(key);
    if (!group) {
      group = {
        key,
        label: `${mode.width} × ${mode.height}${mode.hiDpi ? ' · HiDPI' : ''}`,
        modes: [],
      };
      groups.set(key, group);
    }
    // Quartz can list duplicate timings. Keep the current mode when it is
    // one of them, so the UI never turns an unchanged choice into a write.
    const duplicate = group.modes.findIndex(
      (candidate) => Math.abs(candidate.refreshRate - mode.refreshRate) < 0.001,
    );
    if (duplicate < 0) group.modes.push(mode);
    else if (mode.id === currentModeId) group.modes[duplicate] = mode;
  }
  for (const group of groups.values())
    group.modes = group.modes.toSorted((a, b) => b.refreshRate - a.refreshRate);
  return [...groups.values()].toSorted((a, b) => {
    const first = a.modes[0];
    const second = b.modes[0];
    return first && second
      ? first.width - second.width ||
          first.height - second.height ||
          Number(second.hiDpi) - Number(first.hiDpi)
      : 0;
  });
}

export function preferredMode(
  group: ResolutionGroup,
  current: DisplayMode | undefined,
): DisplayMode | undefined {
  return (
    group.modes.find((mode) => mode.id === current?.id) ??
    group.modes.find(
      (mode) => current && Math.abs(mode.refreshRate - current.refreshRate) < 0.001,
    ) ??
    group.modes[0]
  );
}
