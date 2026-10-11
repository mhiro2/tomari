import type { DisplayConfigurationRequest, DisplayProfile, DisplaySnapshot } from './lib/displays';
import type {
  AppSettings,
  ConfigurationBackupOutcome,
  ConfigurationWarnings,
  DiagnosticsSnapshot,
  Hotkey,
  KeepAwakeOptions,
  KeepAwakeStatus,
  MenuBarInventory,
  ModifierRule,
  PlacementContext,
  SetupStatus,
  WindowPlacement,
} from './lib/types';

type PreviewBackup = {
  formatVersion: number;
  settings: AppSettings;
  hotkeys: Hotkey[];
  modifierRules: ModifierRule[];
  windowPlacements: WindowPlacement[];
  displayProfiles: DisplayProfile[];
};
const counts = (value: PreviewBackup) => ({
  hotkeys: value.hotkeys.length,
  modifierRules: value.modifierRules.length,
  windowPlacements: value.windowPlacements.length,
  displayProfiles: value.displayProfiles.length,
});

// Browser-only sample data for visual review. It is installed only when Vite
// runs in development with ?preview=1; production and the Tauri WebView keep
// using the real IPC bridge.
export async function installDevPreview() {
  const { mockIPC } = await import('@tauri-apps/api/mocks');
  const params = new URLSearchParams(window.location.search);
  const language = params.get('lang') === 'ja' ? 'ja' : 'en';
  const permissionsMissing = params.has('missing');
  const setupPending = params.has('setupPending');
  const showConfigurationWarnings = params.has('invalidConfig');
  let recoveryKind: 'retryable' | 'databaseReset' | null = params.has('databaseReset')
    ? 'databaseReset'
    : params.has('recovery')
      ? 'retryable'
      : null;
  const settings: AppSettings = {
    launchAtLogin: true,
    language,
    keyboardEnabled: true,
    windowManagementEnabled: true,
    externalWindowActionsEnabled: false,
    commandImeSwitchEnabled: true,
    showInMenuBar: true,
    dragToSnapEnabled: true,
    dragToMoveEnabled: false,
    menuBarTidyEnabled: false,
    menuBarAutoCollapseSecs: 15,
  };
  const configurationWarnings: ConfigurationWarnings = {
    invalidHotkeys: showConfigurationWarnings
      ? [
          {
            id: 'legacy-quick-panel',
            label: 'Quick panel',
            reason: 'unsafeGlobalShortcut',
          },
        ]
      : [],
    invalidModifierRules: showConfigurationWarnings
      ? [
          {
            id: 'legacy-command-left',
            label: 'Left Command',
            reason: 'reservedCommandSlot',
          },
        ]
      : [],
    revision: showConfigurationWarnings ? 1 : 0,
  };
  const setup: SetupStatus = {
    firstRun: false,
    updateRegrant: false,
    accessibility: !permissionsMissing,
    inputMonitoring: !permissionsMissing,
    revision: 0,
  };
  const keepAwake: KeepAwakeStatus = {
    active: false,
    lidClose: 'off',
    phase: 'off',
    options: {
      mode: 'idleOnly',
      durationSecs: null,
      endsAtMs: null,
      acOnly: false,
      lowBatteryAction: 'warn',
    },
    notice: null,
    powerSource: 'ac',
    batteryPercent: 86,
    kernelSleepDisabled: false,
    ownsLidClose: false,
    leftoverUndecided: false,
    longRunningProcesses: [
      { pid: 4218, name: 'codex', elapsedSecs: 1_942 },
      { pid: 4330, name: 'cargo', elapsedSecs: 694 },
    ],
    revision: 1,
  };
  const diagnostics: DiagnosticsSnapshot = {
    generatedAtMs: Date.now(),
    app: { version: '0.0.2', os: 'macos', architecture: 'aarch64' },
    permissions: {
      accessibility: !permissionsMissing,
      inputMonitoring: !permissionsMissing,
    },
    taps: [
      {
        kind: 'keyboard',
        enabled: true,
        state: permissionsMissing ? 'permissionDenied' : 'healthy',
        restartCount: 1,
        disableCount: 2,
        recoveryCount: 2,
      },
      {
        kind: 'dragToSnap',
        enabled: true,
        state: permissionsMissing ? 'permissionDenied' : 'healthy',
        restartCount: 0,
        disableCount: 0,
        recoveryCount: 0,
      },
      {
        kind: 'dragToMove',
        enabled: false,
        state: 'stopped',
        restartCount: 0,
        disableCount: 0,
        recoveryCount: 0,
      },
    ],
    capsLock: { ownership: 'held', mappingActive: true, reconciled: true },
    shortcuts: {
      enabled: true,
      registrationIncomplete: false,
      registeredCount: 6,
      invalidCount: configurationWarnings.invalidHotkeys.length,
    },
    menuBar: {
      enabled: false,
      supported: false,
      permissionGranted: !permissionsMissing,
      dividerAvailable: false,
    },
    keepAwake: {
      mode: 'idleOnly',
      active: false,
      phase: 'off',
      markerPresent: false,
      kernelSleepDisabled: false,
      ownsLidClose: false,
    },
    database: { integrityOk: true, schemaVersion: 3, latestSchemaVersion: 3 },
    updater: { signatureConfigured: true },
    privacy: {
      rawInputIncluded: false,
      accessibilityLabelsIncluded: false,
      processDetailsIncluded: false,
      filesystemPathsIncluded: false,
    },
  };
  const modifierRules: ModifierRule[] = [
    {
      id: 'caps',
      label: 'Caps Lock',
      modifier: 'capsLock',
      side: 'either',
      remapTo: 'control',
      hyper: false,
      tap: { type: 'recallWindowPlacement' },
      enabled: true,
    },
    {
      id: 'control-left',
      label: 'Left Control',
      modifier: 'control',
      side: 'left',
      remapTo: 'control',
      hyper: false,
      tap: { type: 'togglePanel' },
      enabled: true,
    },
    {
      id: 'option-left',
      label: 'Left Option',
      modifier: 'option',
      side: 'left',
      remapTo: 'option',
      hyper: false,
      tap: { type: 'noOp' },
      enabled: true,
    },
    {
      id: 'function',
      label: 'fn',
      modifier: 'function',
      side: 'either',
      remapTo: 'function',
      hyper: false,
      tap: { type: 'toggleKeepAwake' },
      enabled: true,
    },
  ];
  if (showConfigurationWarnings) {
    modifierRules.push({
      id: 'legacy-command-left',
      label: 'Left Command',
      modifier: 'command',
      side: 'left',
      remapTo: null,
      hyper: false,
      tap: { type: 'switchIme', value: 'kana' },
      enabled: false,
    });
  }
  const hotkeys: Hotkey[] = [
    {
      id: 'snap-left',
      label: 'Left half',
      accelerator: 'Ctrl+Alt+Left',
      action: { type: 'snapWindow', value: 'leftHalf' },
      enabled: true,
    },
    {
      id: 'snap-right',
      label: 'Right half',
      accelerator: 'Ctrl+Alt+Right',
      action: { type: 'snapWindow', value: 'rightHalf' },
      enabled: true,
    },
    {
      id: 'maximize',
      label: 'Maximize',
      accelerator: 'Ctrl+Alt+Enter',
      action: { type: 'snapWindow', value: 'maximize' },
      enabled: true,
    },
    {
      id: 'restore',
      label: 'Restore position',
      accelerator: 'Ctrl+Alt+R',
      action: { type: 'recallWindowPlacement' },
      enabled: true,
    },
    {
      id: 'move-restore',
      label: 'Next display',
      accelerator: 'Ctrl+Alt+Shift+Right',
      action: { type: 'moveWindowToDisplayAndRecall', value: 'next' },
      enabled: true,
    },
    {
      id: 'panel',
      label: 'Show Tomari',
      accelerator: 'Cmd+Shift+K',
      action: { type: 'togglePanel' },
      enabled: true,
    },
  ];
  const placement: PlacementContext = {
    target: { bundleId: 'com.apple.finder', windowId: 'preview-window' },
    application: { bundleId: 'com.apple.finder', name: 'Finder' },
    currentFrame: { x: 0.1, y: 0.08, width: 0.54, height: 0.8 },
    placements: [
      {
        application: { bundleId: 'com.apple.finder', name: 'Finder' },
        slot: 'primary',
        frame: { x: 0, y: 0.04, width: 0.5, height: 0.92 },
      },
      {
        application: { bundleId: 'com.apple.finder', name: 'Finder' },
        slot: 'secondary',
        frame: { x: 0.5, y: 0.04, width: 0.5, height: 0.92 },
      },
    ],
    damagedPlacements: [],
    canMoveToDisplay: true,
  };
  const menuItems: MenuBarInventory = {
    supported: false,
    permissionGranted: !permissionsMissing,
    dividerAvailable: false,
    items: [],
  };

  const snapshot = (): PreviewBackup =>
    structuredClone({
      formatVersion: 2,
      settings,
      hotkeys,
      modifierRules,
      windowPlacements: placement.placements,
      displayProfiles,
    });
  let previousConfiguration: PreviewBackup | null = null;
  let pendingBackup: {
    token: number;
    kind: 'import' | 'restore';
    before: PreviewBackup;
    incoming: PreviewBackup;
  } | null = null;
  let lastApplied: (ConfigurationBackupOutcome & { token: number }) | null = null;
  let backupToken = 0;
  function previewBackup(incoming: PreviewBackup, kind: 'import' | 'restore') {
    const before = snapshot();
    pendingBackup = { token: ++backupToken, kind, before, incoming };
    return {
      token: pendingBackup.token,
      kind,
      current: counts(before),
      incoming: counts(incoming),
      currentSettings: before.settings,
      settings: incoming.settings,
    };
  }

  const retina = {
    id: 1,
    width: 2560,
    height: 1440,
    pixelWidth: 5120,
    pixelHeight: 2880,
    hiDpi: true,
    refreshRate: 60,
  };
  const larger = {
    ...retina,
    id: 2,
    width: 1920,
    height: 1080,
    pixelWidth: 3840,
    pixelHeight: 2160,
  };
  const laptop = {
    id: 3,
    width: 1512,
    height: 982,
    pixelWidth: 3024,
    pixelHeight: 1964,
    hiDpi: true,
    refreshRate: 0,
  };
  const displaySnapshot: DisplaySnapshot = {
    revision: 0,
    pending: null,
    notice: null,
    displays: [
      {
        id: 'studio',
        connectionId: 1,
        name: 'Studio Display',
        builtIn: false,
        main: true,
        mirrored: false,
        x: 0,
        y: 0,
        width: 2560,
        height: 1440,
        currentMode: retina,
        modes: [
          larger,
          retina,
          { ...retina, id: 4, hiDpi: false, pixelWidth: 2560, pixelHeight: 1440 },
        ],
        brightness: 0.72,
        softwareDimming: 0,
      },
      {
        id: 'macbook',
        connectionId: 2,
        name: 'Built-in display',
        builtIn: true,
        main: false,
        mirrored: false,
        x: -1512,
        y: 458,
        width: 1512,
        height: 982,
        currentMode: laptop,
        modes: [laptop],
        brightness: 0.5,
        softwareDimming: 0,
      },
    ],
  };
  let displayProfiles: DisplayProfile[] = [];
  let displayDeadline = 0;
  let previousDisplays = structuredClone(displaySnapshot.displays);
  function readPreviewDisplays() {
    if (displaySnapshot.pending) {
      displaySnapshot.pending.remainingMs = Math.max(0, displayDeadline - Date.now());
      if (displaySnapshot.pending.remainingMs === 0) {
        displaySnapshot.displays = previousDisplays;
        displaySnapshot.pending = null;
        displaySnapshot.notice = 'reverted';
      }
    }
    return structuredClone(displaySnapshot);
  }

  const previewCommand: Parameters<typeof mockIPC>[0] = (cmd, payload) => {
    switch (cmd) {
      case 'get_displays':
        return readPreviewDisplays();
      case 'get_display_navigation':
        return null;
      case 'identify_displays':
      case 'open_system_display_settings':
        return;
      case 'acknowledge_display_navigation':
        return;
      case 'get_display_profiles':
        return structuredClone(displayProfiles);
      case 'save_display_profile': {
        const { name } = payload as { name: string };
        const id = crypto.randomUUID();
        displayProfiles.push({
          id,
          name: name.trim(),
          displays: displaySnapshot.displays.map((display) => {
            const mode = display.currentMode;
            if (!mode) throw new Error('Current mode unavailable');
            return {
              displayId: display.id,
              x: display.x,
              y: display.y,
              mode: {
                width: mode.width,
                height: mode.height,
                pixelWidth: mode.pixelWidth,
                pixelHeight: mode.pixelHeight,
                refreshRate: mode.refreshRate,
              },
            };
          }),
        });
        return id;
      }
      case 'rename_display_profile': {
        const { id, name } = payload as { id: string; name: string };
        const profile = displayProfiles.find((item) => item.id === id);
        if (!profile) throw new Error('Profile unavailable');
        profile.name = name.trim();
        return;
      }
      case 'delete_display_profile': {
        const { id } = payload as { id: string };
        displayProfiles = displayProfiles.filter((profile) => profile.id !== id);
        return;
      }
      case 'clear_display_profiles':
        displayProfiles = [];
        return;
      case 'preview_display_profile': {
        const { id } = payload as { id: string };
        const profile = displayProfiles.find((item) => item.id === id);
        if (!profile || profile.displays.length !== displaySnapshot.displays.length)
          throw new Error('Profile unavailable');
        previousDisplays = structuredClone(displaySnapshot.displays);
        const byId = new Map(displaySnapshot.displays.map((display) => [display.id, display]));
        for (const saved of profile.displays) {
          const display = byId.get(saved.displayId);
          const mode = display?.modes.find(
            (candidate) =>
              candidate.width === saved.mode.width &&
              candidate.height === saved.mode.height &&
              candidate.pixelWidth === saved.mode.pixelWidth &&
              candidate.pixelHeight === saved.mode.pixelHeight &&
              Math.abs(candidate.refreshRate - saved.mode.refreshRate) < 0.01,
          );
          if (!display || !mode) throw new Error('Profile mode unavailable');
          display.currentMode = mode;
          display.width = mode.width;
          display.height = mode.height;
          display.x = saved.x;
          display.y = saved.y;
          display.main = saved.x === 0 && saved.y === 0;
        }
        displaySnapshot.revision += 1;
        displaySnapshot.notice = null;
        displayDeadline = Date.now() + 20000;
        displaySnapshot.pending = {
          token: displaySnapshot.revision,
          displayId: profile.displays[0]?.displayId ?? '',
          remainingMs: 20000,
        };
        return readPreviewDisplays();
      }
      case 'preview_display_configuration': {
        const { request } = payload as { request: DisplayConfigurationRequest };
        const display = displaySnapshot.displays.find((item) => item.id === request.displayId);
        if (!display) throw new Error('Display unavailable');
        previousDisplays = structuredClone(displaySnapshot.displays);
        if (request.kind === 'mode') {
          const mode = display.modes.find((item) => item.id === request.modeId);
          if (!mode) throw new Error('Display mode unavailable');
          display.currentMode = mode;
          display.width = mode.width;
          display.height = mode.height;
        } else if (request.kind === 'main') {
          const x = display.x;
          const y = display.y;
          for (const item of displaySnapshot.displays) {
            item.x -= x;
            item.y -= y;
            item.main = item.id === display.id;
          }
        } else {
          const anchor = displaySnapshot.displays.find((item) => item.id === request.relativeTo);
          if (!anchor) throw new Error('Display unavailable');
          if (request.placement === 'left' || request.placement === 'right') {
            display.x =
              request.placement === 'left' ? anchor.x - display.width : anchor.x + anchor.width;
            display.y = anchor.y + Math.trunc((anchor.height - display.height) / 2);
          } else {
            display.x = anchor.x + Math.trunc((anchor.width - display.width) / 2);
            display.y =
              request.placement === 'above' ? anchor.y - display.height : anchor.y + anchor.height;
          }
          if (display.main) {
            const x = display.x;
            const y = display.y;
            for (const item of displaySnapshot.displays) {
              item.x -= x;
              item.y -= y;
            }
          }
        }
        displaySnapshot.revision += 1;
        displaySnapshot.notice = null;
        displayDeadline = Date.now() + 20000;
        displaySnapshot.pending = {
          token: displaySnapshot.revision,
          displayId: display.id,
          remainingMs: 20000,
        };
        return readPreviewDisplays();
      }
      case 'confirm_display_configuration':
        displaySnapshot.pending = null;
        displaySnapshot.revision += 1;
        return readPreviewDisplays();
      case 'revert_display_configuration':
        displayDeadline = 0;
        return readPreviewDisplays();
      case 'set_display_dimming': {
        const { displayId, amount } = payload as { displayId: string; amount: number };
        const display = displaySnapshot.displays.find((item) => item.id === displayId);
        if (display) display.softwareDimming = amount;
        return;
      }
      case 'reset_display_dimming':
        for (const display of displaySnapshot.displays) display.softwareDimming = 0;
        return;
      case 'set_display_brightness': {
        const { displayId, value } = payload as { displayId: string; value: number };
        const display = displaySnapshot.displays.find((item) => item.id === displayId);
        if (display) display.brightness = value;
        displaySnapshot.revision += 1;
        return readPreviewDisplays();
      }
      case 'restart_app':
        throw { code: 'other', message: 'Restart is available in the macOS app.' };
      case 'get_settings': {
        if (recoveryKind !== null) {
          throw {
            code:
              recoveryKind === 'databaseReset'
                ? 'databaseResetRequired'
                : 'settingsRecoveryRequired',
            message:
              recoveryKind === 'databaseReset'
                ? 'settings database was quarantined'
                : 'saved settings could not be read',
          };
        }
        return settings;
      }
      case 'get_configuration_warnings':
        return configurationWarnings;
      case 'get_diagnostics':
        return {
          ...diagnostics,
          generatedAtMs: Date.now(),
          keepAwake: {
            mode: keepAwake.options.mode,
            active: keepAwake.active,
            phase: keepAwake.phase,
            markerPresent: keepAwake.ownsLidClose,
            kernelSleepDisabled: keepAwake.kernelSleepDisabled,
            ownsLidClose: keepAwake.ownsLidClose,
          },
        };
      case 'get_apply_warnings':
        return { warnings: [], unprobed: ['menuBar'] };
      case 'get_configuration_backup_status':
        return { canRestore: previousConfiguration !== null, lastApplied };
      case 'export_configuration_backup':
        return {
          path: '/Users/preview/Library/Application Support/Tomari/backups/tomari-settings.json',
          generatedAtMs: Date.now(),
        };
      case 'reveal_configuration_backups':
        return null;
      case 'preview_configuration_import': {
        const { contents } = payload as { contents: string };
        // Only visual fixtures run here; the real Rust boundary owns complete
        // structural and semantic validation of user-selected files.
        const incoming = JSON.parse(contents) as PreviewBackup;
        if (
          incoming.formatVersion !== 2 ||
          !incoming.settings ||
          !Array.isArray(incoming.hotkeys) ||
          !Array.isArray(incoming.modifierRules) ||
          !Array.isArray(incoming.windowPlacements) ||
          !Array.isArray(incoming.displayProfiles)
        ) {
          throw { code: 'backupInvalid', message: 'Invalid preview backup' };
        }
        return previewBackup(incoming, 'import');
      }
      case 'preview_configuration_restore':
        if (!previousConfiguration)
          throw { code: 'backupUnavailable', message: 'No previous settings' };
        return previewBackup(previousConfiguration, 'restore');
      case 'apply_configuration_backup': {
        const { token } = payload as { token: number };
        if (
          !pendingBackup ||
          token !== pendingBackup.token ||
          JSON.stringify(pendingBackup.before) !== JSON.stringify(snapshot())
        ) {
          throw { code: 'backupPreviewExpired', message: 'Review the settings again' };
        }
        const { before, incoming, kind } = pendingBackup;
        previousConfiguration = kind === 'import' ? before : null;
        Object.assign(settings, incoming.settings);
        hotkeys.splice(0, hotkeys.length, ...incoming.hotkeys);
        modifierRules.splice(0, modifierRules.length, ...incoming.modifierRules);
        placement.placements = incoming.windowPlacements;
        displayProfiles = incoming.displayProfiles;
        pendingBackup = null;
        lastApplied = { token, settings: { ...settings }, applyWarnings: [] };
        return lastApplied;
      }
      case 'export_support_bundle':
        return {
          path: '/Users/preview/Library/Application Support/Tomari/support/tomari-support.json',
          generatedAtMs: diagnostics.generatedAtMs,
        };
      case 'retry_settings_recovery':
        recoveryKind = null;
        return null;
      case 'reset_settings_recovery':
        Object.assign(settings, {
          launchAtLogin: false,
          keyboardEnabled: false,
          windowManagementEnabled: false,
          externalWindowActionsEnabled: false,
          commandImeSwitchEnabled: false,
          showInMenuBar: true,
          dragToSnapEnabled: false,
          dragToMoveEnabled: false,
          menuBarTidyEnabled: false,
          menuBarAutoCollapseSecs: 0,
        });
        recoveryKind = null;
        return null;
      case 'save_settings':
        Object.assign(settings, (payload as { settings: AppSettings }).settings);
        return { applyWarnings: [] };
      case 'setup_status':
        if (setupPending) return new Promise<SetupStatus>(() => {});
        return setup;
      case 'input_monitoring_status':
      case 'accessibility_status':
        return true;
      case 'get_keep_awake':
        return structuredClone(keepAwake);
      case 'set_keep_awake': {
        const { enabled, options } = payload as { enabled: boolean; options?: KeepAwakeOptions };
        if (options) keepAwake.options = { ...options };
        keepAwake.active = enabled;
        keepAwake.phase = enabled ? 'on' : 'off';
        keepAwake.ownsLidClose = enabled && keepAwake.options.mode === 'lidClosed';
        keepAwake.lidClose = keepAwake.ownsLidClose ? 'engaged' : 'off';
        keepAwake.kernelSleepDisabled = keepAwake.ownsLidClose;
        if (enabled && keepAwake.options.durationSecs !== null) {
          keepAwake.options.endsAtMs = Date.now() + keepAwake.options.durationSecs * 1_000;
        }
        keepAwake.notice = null;
        keepAwake.revision += 1;
        return structuredClone(keepAwake);
      }
      case 'configure_keep_awake': {
        const { options } = payload as { options: KeepAwakeOptions };
        if (keepAwake.active && options.mode !== keepAwake.options.mode) {
          throw { code: 'invalidInput', message: 'Stop the session before changing modes.' };
        }
        keepAwake.options = { ...options };
        if (keepAwake.active && options.durationSecs !== null) {
          keepAwake.options.endsAtMs = Date.now() + options.durationSecs * 1_000;
        }
        keepAwake.revision += 1;
        return structuredClone(keepAwake);
      }
      case 'cancel_keep_awake_transition':
      case 'retry_keep_awake_transition':
        return structuredClone(keepAwake);
      case 'get_menu_bar':
        return { enabled: false, collapsed: false, supported: false };
      case 'list_modifier_rules':
        return modifierRules;
      case 'save_modifier_rule': {
        const { rule, originalId } = payload as { rule: ModifierRule; originalId: string | null };
        const saved = { ...rule, id: rule.id.trim(), label: rule.label.trim() };
        const index = modifierRules.findIndex((candidate) => candidate.id === originalId);
        if (index === -1) modifierRules.push(saved);
        else modifierRules.splice(index, 1, saved);
        configurationWarnings.invalidModifierRules =
          configurationWarnings.invalidModifierRules.filter((issue) => issue.id !== originalId);
        configurationWarnings.revision += 1;
        return { rule: saved, applyWarnings: [] };
      }
      case 'delete_modifier_rule': {
        const { id } = payload as { id: string };
        const index = modifierRules.findIndex((rule) => rule.id === id);
        if (index !== -1) modifierRules.splice(index, 1);
        configurationWarnings.invalidModifierRules =
          configurationWarnings.invalidModifierRules.filter((issue) => issue.id !== id);
        configurationWarnings.revision += 1;
        return { applyWarnings: [] };
      }
      case 'dismiss_keep_awake_recovery':
        return keepAwake;
      case 'list_hotkeys':
        return hotkeys;
      case 'save_hotkey': {
        const { hotkey } = payload as { hotkey: Hotkey };
        const saved = { ...hotkey, id: hotkey.id.trim(), label: hotkey.label.trim() };
        const index = hotkeys.findIndex(
          (candidate) => candidate.id === hotkey.id || candidate.id === saved.id,
        );
        if (index === -1) hotkeys.push(saved);
        else hotkeys.splice(index, 1, saved);
        return saved;
      }
      case 'delete_hotkey': {
        const { id } = payload as { id: string };
        const index = hotkeys.findIndex((hotkey) => hotkey.id === id);
        if (index !== -1) hotkeys.splice(index, 1);
        return null;
      }
      case 'get_placement_context':
        return placement;
      case 'get_window_history_status':
        return { canUndo: true, canRedo: false };
      case 'list_menu_bar_items':
        return menuItems;
      case 'move_menu_bar_item':
        return { outcome: 'unsupported', inventory: menuItems };
      case 'validate_accelerator': {
        const { accelerator } = payload as { accelerator: string };
        // Recording already produces canonical chords; semantic validation is
        // exercised against the Rust command, never this visual preview.
        return { valid: true, normalized: accelerator, error: null };
      }
      case 'plugin:app|version':
        return '0.0.2';
      default:
        return null;
    }
  };
  mockIPC(previewCommand, { shouldMockEvents: true });
}
