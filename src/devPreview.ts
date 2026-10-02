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
};
const counts = (value: PreviewBackup) => ({
  hotkeys: value.hotkeys.length,
  modifierRules: value.modifierRules.length,
  windowPlacements: value.windowPlacements.length,
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
      formatVersion: 1,
      settings,
      hotkeys,
      modifierRules,
      windowPlacements: placement.placements,
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

  const previewCommand: Parameters<typeof mockIPC>[0] = (cmd, payload) => {
    switch (cmd) {
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
          incoming.formatVersion !== 1 ||
          !incoming.settings ||
          !Array.isArray(incoming.hotkeys) ||
          !Array.isArray(incoming.modifierRules) ||
          !Array.isArray(incoming.windowPlacements)
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
