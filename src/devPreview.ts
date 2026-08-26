import type {
  AppSettings,
  Hotkey,
  KeepAwakeStatus,
  MenuBarInventory,
  ModifierRule,
  PlacementContext,
  SetupStatus,
} from './lib/types';

// Browser-only sample data for visual review. It is installed only when Vite
// runs in development with ?preview=1; production and the Tauri WebView keep
// using the real IPC bridge.
export async function installDevPreview() {
  const { mockIPC } = await import('@tauri-apps/api/mocks');
  const params = new URLSearchParams(window.location.search);
  const language = params.get('lang') === 'ja' ? 'ja' : 'en';
  const permissionsMissing = params.has('missing');
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
    menuBarTidyEnabled: true,
    menuBarAutoCollapseSecs: 15,
  };
  const setup: SetupStatus = {
    firstRun: false,
    updateRegrant: false,
    accessibility: !permissionsMissing,
    inputMonitoring: !permissionsMissing,
  };
  const keepAwake: KeepAwakeStatus = {
    active: false,
    lidClose: 'off',
    phase: 'off',
    options: {
      durationSecs: null,
      endsAtMs: null,
      acOnly: false,
      lowBatteryAction: 'warn',
    },
    notice: null,
    revision: 1,
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
    canMoveToDisplay: true,
  };
  let menuItemGeneration = 1;
  let menuItems: MenuBarInventory = {
    supported: true,
    permissionGranted: true,
    dividerAvailable: true,
    items: [
      { id: '1:0', name: 'Dropbox', ownerName: 'Dropbox', bundleId: null, zone: 'hidden' },
      {
        id: '1:1',
        name: 'Docker',
        ownerName: 'Docker Desktop',
        bundleId: null,
        zone: 'hidden',
      },
      { id: '1:2', name: 'VPN', ownerName: 'VPN', bundleId: null, zone: 'hidden' },
      {
        id: '1:3',
        name: 'Wi-Fi',
        ownerName: 'Control Center',
        bundleId: 'com.apple.controlcenter',
        zone: 'visible',
      },
      {
        id: '1:4',
        name: 'Battery',
        ownerName: 'Control Center',
        bundleId: 'com.apple.controlcenter',
        zone: 'visible',
      },
      {
        id: '1:5',
        name: 'Clock',
        ownerName: 'Control Center',
        bundleId: 'com.apple.controlcenter',
        zone: 'visible',
      },
    ],
  };

  mockIPC((cmd, payload) => {
    switch (cmd) {
      case 'get_settings':
        return settings;
      case 'save_settings':
        return { applyWarnings: [] };
      case 'setup_status':
        return setup;
      case 'input_monitoring_status':
      case 'accessibility_status':
        return true;
      case 'get_keep_awake':
      case 'set_keep_awake':
      case 'configure_keep_awake':
      case 'cancel_keep_awake_transition':
      case 'retry_keep_awake_transition':
        return keepAwake;
      case 'get_menu_bar':
        return { enabled: true, collapsed: true };
      case 'list_modifier_rules':
        return modifierRules;
      case 'list_hotkeys':
        return hotkeys;
      case 'get_placement_context':
        return placement;
      case 'get_window_history_status':
        return { canUndo: true, canRedo: false };
      case 'list_menu_bar_items':
        return menuItems;
      case 'move_menu_bar_item': {
        const { itemId, targetZone } = payload as {
          itemId: string;
          targetZone: 'hidden' | 'visible';
        };
        const item = menuItems.items.find((candidate) => candidate.id === itemId);
        const outcome = !item ? 'staleItem' : item.zone === targetZone ? 'alreadyInZone' : 'moved';
        menuItemGeneration += 1;
        menuItems = {
          ...menuItems,
          items: menuItems.items.map((candidate, index) => ({
            ...candidate,
            id: `${menuItemGeneration}:${index}`,
            zone: candidate.id === itemId ? targetZone : candidate.zone,
          })),
        };
        return { outcome, inventory: menuItems };
      }
      case 'plugin:app|version':
        return '0.0.1';
      default:
        return null;
    }
  });
}
