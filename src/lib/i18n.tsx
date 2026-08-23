// Minimal i18n: a typed English dictionary, a Japanese mirror enforced by the
// type checker, and a context that hands components a translate function.

import { createContext, useContext, type ReactNode } from 'react';

import type { Language } from './types';

const en = {
  // Sidebar section names. Prevent Sleep is named for what it does, matching
  // the tray entry and the switch inside the section.
  'app.nav.keyboard': 'Keyboard',
  'app.nav.window': 'Windows',
  'app.nav.menubar': 'Menu Bar',
  'app.nav.session': 'Prevent Sleep',
  'app.nav.general': 'General',
  'app.sections': 'Sections',
  'app.tools': 'Tools',
  'app.app': 'App',
  'app.permissionsReady': 'Permissions: Ready',
  'app.permissionsAttention': 'Permissions: Needs attention',

  'common.add': 'Add',
  'common.delete': 'Delete',
  'common.deleteConfirm': 'Delete {label}?',
  'common.deleteConfirmShort': 'Delete?',
  'common.cancel': 'Cancel',
  'common.empty': 'Nothing here yet.',
  'common.label': 'Label',
  'common.loading': 'Loading…',
  'common.on': 'On',
  'common.off': 'Off',
  'common.enable': 'Enable {label}',
  'common.turnOn': 'Turn On',
  'common.loadFailed': 'Could not load settings: {error}',
  'common.retry': 'Retry',
  'common.refresh': 'Refresh',
  'common.undo': 'Undo',
  'common.redo': 'Redo',
  'common.forget': 'Forget',

  'preset.leftHalf': 'Left Half',
  'preset.rightHalf': 'Right Half',
  'preset.topHalf': 'Top Half',
  'preset.bottomHalf': 'Bottom Half',
  'preset.topLeftQuarter': 'Top Left',
  'preset.topRightQuarter': 'Top Right',
  'preset.bottomLeftQuarter': 'Bottom Left',
  'preset.bottomRightQuarter': 'Bottom Right',
  'preset.leftThird': 'Left ⅓',
  'preset.centerThird': 'Center ⅓',
  'preset.rightThird': 'Right ⅓',
  'preset.leftTwoThirds': 'Left ⅔',
  'preset.rightTwoThirds': 'Right ⅔',
  'preset.center': 'Center',
  'preset.maximize': 'Maximize',

  'side.left': 'Left',
  'side.right': 'Right',
  'side.either': 'Either',

  'direction.next': 'Next Display',
  'direction.prev': 'Previous Display',

  'action.togglePanel': 'Toggle Tomari',
  'action.snap': 'Snap: {target}',
  'action.moveToDisplay': 'Move to {display}',
  'action.recallPlacement': 'Restore Saved Position',
  'action.moveAndRecall': 'Move & Restore on {display}',
  'action.undoWindow': 'Undo Window Change',
  'action.redoWindow': 'Redo Window Change',
  'action.sendKeystroke': 'Send: {keys}',
  'action.ime': 'IME → {mode}',
  'action.toggleKeepAwake': 'Toggle Prevent Sleep',
  'action.toggleMenuBar': 'Show/Hide Menu Bar Icons',
  'action.noOp': 'Do Nothing',

  'keyboard.modifierKeys': 'Tap and hold',
  'keyboard.pageDescription':
    'Choose what a key does when tapped, while keeping its usual hold behavior.',
  'keyboard.tabsLabel': 'Keyboard settings',
  'keyboard.tab.modifiers': 'Modifier Keys',
  'keyboard.tab.shortcuts': 'Shortcuts',
  'keyboard.table.key': 'Key',
  'keyboard.table.tap': 'Tap',
  'keyboard.table.hold': 'Hold',
  'keyboard.table.enabled': 'On',
  'keyboard.inputSwitching': 'Input switching',
  'keyboard.leftCommand': 'Left Command',
  'keyboard.rightCommand': 'Right Command',
  'keyboard.imeEisu': 'English',
  'keyboard.imeKana': 'Japanese',
  'keyboard.addShortcut': 'Add Shortcut',
  'keyboard.cancelAddShortcut': 'Cancel adding shortcut',
  'keyboard.globalShortcuts': 'Shortcuts for Tomari',
  'keyboard.usedAs': 'Used as {modifier}',
  'keyboard.usedAsHyper': 'Used as Hyper (⌃⌥⇧⌘)',
  'keyboard.tapFor': 'Tap for {action}',
  'keyboard.tapHold': 'Tap for {action}, hold for {modifier}',
  'keyboard.tapAction': 'Tap action',
  'keyboard.tapActionFor': 'Tap action for {modifier}',
  'keyboard.commandImeSwitch': 'Command-key IME switch',
  'keyboard.commandImeSwitchDesc': 'Tap left ⌘ for 英数, right ⌘ for かな.',
  'keyboard.commandImeSwitchNote':
    'Only a solo tap is replaced; when pressed with another key, Command behaves normally.',
  'keyboard.shortcutLabelAria': 'Shortcut label',
  'keyboard.actionAria': 'Action',
  'keyboard.recordShortcut': 'Record Shortcut',
  'keyboard.changeShortcut': 'Change shortcut for {label}',
  'keyboard.deleteShortcut': 'Delete {label}',
  'keyboard.offNote': "Keyboard customization is off — taps and shortcuts won't fire.",
  'keyboard.imNeeded': 'Input Monitoring access needed',
  'keyboard.imBody': 'Needed to detect taps, holds, and the hyper key.',
  'keyboard.noModifierRules': 'No modifier keys to configure.',
  'keyboard.noHotkeys': 'No global shortcuts yet. Use Add Shortcut to create one.',

  'recorder.click': 'Click to record',
  'recorder.typing': 'Type shortcut…',
  'recorder.needModifier': 'Add a modifier key',
  'recorder.unsupported': 'Unsupported shortcut',
  'recorder.startFailed': 'Could not start recording',

  'error.permissionRequired': 'Accessibility permission is required.',
  'error.noFocusedWindow': "There's no window to act on.",
  'error.shortcutConflict':
    "That shortcut couldn't be registered — it may conflict with another app.",
  'error.placementNotFound': 'Remember a position for this app first.',
  'error.windowTargetChanged': 'The focused window changed. The panel has been refreshed.',
  'error.windowNotResponding':
    "The app didn't respond to window control. Wait a moment, then refresh.",

  'setup.title': 'Get Tomari ready',
  'setup.intro':
    'Allow two macOS permissions so Tomari can move windows and recognize key gestures. You can review either permission later.',
  'setup.accessibility': 'Accessibility',
  'setup.accessibilityWhy':
    'Lets Tomari move and resize windows, send keys, and read your menu bar arrangement.',
  'setup.inputMonitoring': 'Input Monitoring',
  'setup.inputMonitoringWhy':
    'Lets Tomari recognize modifier-key taps and holds, the hyper key, and window drag gestures.',
  'setup.granted': 'Granted',
  'setup.grant': 'Open System Settings',
  'setup.grantFor': 'Open System Settings for {name}',
  'setup.requesting': 'Opening…',
  'setup.openAgain': 'Open Again',
  'setup.returnHint': 'After allowing it in System Settings, return to Tomari.',
  'setup.later': 'Set up later',
  'setup.done': 'Start using Tomari',
  'setup.allSet': 'All set — Tomari is ready to use.',
  'setup.tryIt': 'Try it: press {keys} to snap the window you were just using to the left half.',
  'setup.bannerText.one': 'One macOS permission is still needed',
  'setup.bannerText.two': 'Two macOS permissions are still needed',
  'setup.updateBannerText': 'Permissions need attention after the update',
  'setup.bannerDescription.both':
    'Window controls and key gestures stay limited until they are allowed.',
  'setup.bannerDescription.accessibility':
    'Window controls, key sending, and menu bar inspection stay limited until it is allowed.',
  'setup.bannerDescription.inputMonitoring':
    'Modifier-key gestures and window-drag tools stay limited until it is allowed.',
  'setup.bannerAction': 'Review permissions',
  'setup.openSetup': 'Open Setup',
  'setup.updateRegrant':
    "Tomari's permissions went missing after the update — a known limitation for now. Please grant them again.",
  'setup.adminNote':
    'Prevent Sleep is separate: it asks for your administrator password each time instead of a one-time grant.',

  'window.axNeeded': 'Accessibility access needed',
  'window.pageDescription': 'Set saved positions and the shortcuts or gestures that move windows.',
  'window.tabsLabel': 'Window settings',
  'window.tab.saved': 'Saved Positions',
  'window.tab.shortcuts': 'Shortcuts',
  'window.tab.mouse': 'Mouse',
  'window.currentWindow': 'Current window',
  'window.basicShortcuts': 'Common shortcuts',
  'window.moreShortcuts': 'Other layouts ({count})',
  'window.showMoreShortcuts': 'Show other layouts',
  'window.hideMoreShortcuts': 'Hide other layouts',
  'window.addShortcut': 'Add Shortcut',
  'window.dragGesture': 'Drag to an edge',
  'window.resizeGesture': 'Move or resize with modifiers',
  'window.axBody': 'Needed to move and resize other apps’ windows.',
  'window.grantAccess': 'Open System Settings',
  'window.offNote': "Window management is off — snapping and display moves won't work.",
  'window.focusedApp': 'Focused app',
  'window.noFocusedApp': 'Choose a window, then refresh',
  'window.restoreHome': 'Restore saved position',
  'window.moveAndRestore': 'Next display & restore',
  'window.restoredSlot': 'Restored {app} to {slot}',
  'window.movedAndRestoredSlot': 'Moved {app} and restored {slot}',
  'window.noAdjacentDisplay': 'No other display is available',
  'window.undone': 'Window change undone',
  'window.redone': 'Window change redone',
  'window.historyEmpty': 'There is no window change to apply',
  'window.staleHistoryDiscarded': 'Closed-window history was discarded',
  'window.rememberedHomes': 'Saved positions for this app',
  'window.slot.primary': 'Position A',
  'window.slot.secondary': 'Position B',
  'window.rememberedCount': 'Remembered positions: {count}',
  'window.previewAria':
    '{app} window position preview. Remembered position count: {count}. Solid amber is the current position; outlined frames are remembered positions.',
  'window.previewEmptyAria': 'No focused window position to preview',
  'window.currentPosition': 'Current',
  'window.homeReady': 'Works on any display',
  'window.homeEmpty': 'No position remembered',
  'window.lastRestored': 'Last restored',
  'window.rememberHere': 'Save current position',
  'window.replaceHome': 'Replace position',
  'window.remembered': 'Remembered {slot}',
  'window.alreadyRemembered': '{slot} already matches this position',
  'window.forgotten': 'Forgot {slot}',
  'window.savedEditUndone': 'Remembered position restored',
  'window.noSavedEditToUndo': 'There is no remembered-position edit to undo',
  'window.confirmForget': 'Forget?',
  'window.confirmForgetAria': 'Forget {slot}?',
  'window.forgetAria': 'Forget {slot}',
  'window.controls': 'Controls',
  'window.modifierTapActions': 'Modifier-key tap actions',
  'window.modifierTapActionsHint':
    'Optionally assign Restore to a Caps Lock or other modifier-key tap in Keyboard.',
  'window.modifierTapActionsDisabled':
    'Turn on Keyboard customization to use these shortcuts or assign Restore to a modifier-key tap.',
  'window.openKeyboard': 'Open Keyboard',
  'window.noShortcuts': 'No window shortcuts yet. Use Add Shortcut to create one.',
  'window.mouse': 'Mouse',
  'window.mouseDescription':
    'Choose whether dragging near an edge—or dragging with modifier keys—moves a window for you.',
  'window.dragToSnapToggle': 'Snap by dragging to a screen edge',
  'window.enableDragToSnap': 'Enable Drag to Snap',
  'window.dragToSnapHint': 'Drag to an edge or corner, then release when the preview appears.',

  'window.dragToMoveToggle': 'Move or resize the window under the pointer',
  'window.enableDragToMove': 'Enable Drag to Move & Resize',
  'window.dragToMoveHint': 'Hold ⌃⌥ and drag to move. Add ⌘ to resize.',

  'menubar.title': 'Menu Bar',
  'menubar.pageDescription':
    'Choose which icons stay visible and how the hidden area opens and closes.',
  'menubar.tabsLabel': 'Menu bar settings',
  'menubar.tab.items': 'Items',
  'menubar.tab.behavior': 'Behavior',
  'menubar.diagramLabel': 'Current menu bar arrangement',
  'menubar.arrangeInstruction': 'Use the buttons below to move items across Tomari’s divider.',
  'menubar.offNote': 'Menu bar tidying is off — Tomari adds no divider to your menu bar.',
  'menubar.enable': 'Turn on menu bar tidying',
  'menubar.showToggle': 'Show hidden icons',
  'menubar.showDesc': 'Also from the ‹ item, the tray menu, or a shortcut.',
  'menubar.iconsHidden': 'Hidden icons are tucked away',
  'menubar.iconsVisible': 'Hidden icons are visible now',
  'menubar.showAction': 'Show icons',
  'menubar.hideAction': 'Hide again',
  'menubar.autoCollapse': 'Collapse automatically',
  'menubar.autoCollapseNever': 'Never',
  'menubar.autoCollapseSecs': 'After {secs} seconds',
  'menubar.behaviorSection': 'Open and close',
  'menubar.arrangeSection': 'Choosing what to hide',
  'menubar.arrangeBody':
    'Hold ⌘ and drag your menu bar icons so the ones you want tucked away sit left of the ≡ divider.',
  'menubar.inventoryLoading': 'Reading menu bar items…',
  'menubar.inventoryEmpty': 'No items detected in this section.',
  'menubar.inventoryError': 'Could not read the menu bar. Try refreshing after expanding it.',
  'menubar.inventoryUnsupported': 'Menu bar inspection is available on macOS.',
  'menubar.inventoryDividerMissing':
    'The divider is not available yet. Turn menu bar tidying off and on, then refresh.',
  'menubar.inventoryPermission': 'Accessibility access is required to identify menu bar items.',
  'menubar.grantAccessibility': 'Grant Access…',
  'menubar.refreshItems': 'Refresh Items',
  'menubar.hiddenItems': 'Hidden now',
  'menubar.visibleItems': 'Always shown',
  'menubar.itemCount': '{count}',
  'menubar.itemOwner': 'From {owner}',
  'menubar.zoneHidden': 'hidden',
  'menubar.zoneVisible': 'always shown',
  'menubar.moveHide': 'Hide',
  'menubar.moveShow': 'Always Show',
  'menubar.moveHideItem': 'Hide {item}',
  'menubar.moveShowItem': 'Always show {item}',
  'menubar.moving': 'Moving…',
  'menubar.movingItem': 'Moving {item}…',
  'menubar.moveStale':
    'The menu bar changed before Tomari could move {item}. The list has been refreshed; try again.',
  'menubar.moveFailed': 'Tomari could not move {item}.',
  'menubar.moveManualFallback':
    'Hold ⌘ and drag it across the divider in the real menu bar instead.',

  'settings.general': 'General',
  'settings.pageDescription':
    'Choose when Tomari starts, where it appears, and whether other apps may control it.',
  'settings.startup': 'Startup and appearance',
  'settings.startupDescription': 'Set how you find Tomari and which language it uses.',
  'settings.launchAtLogin': 'Launch at login',
  'settings.launchAtLoginHint': 'Start Tomari automatically when you sign in to your Mac.',
  'settings.showInMenuBar': 'Show in menu bar',
  'settings.showInMenuBarHint': 'Keep the Tomari icon visible for quick access.',
  'settings.hiddenHint':
    'Hidden from the menu bar — reopen Tomari any time by launching it again from Spotlight or Launchpad, or with the global shortcut (default ⌘⇧K).',
  'settings.hideTrayConfirmTitle': 'Hide the menu bar icon?',
  'settings.hideTrayConfirmBody':
    'Tomari keeps running with no menu bar icon and no Dock icon. To open it again, launch Tomari from Spotlight or Launchpad, or use the global shortcut (default ⌘⇧K).',
  'settings.hideTrayConfirmAction': 'Hide Icon',
  'settings.language': 'Language',
  'settings.languageHint': 'Changes the language of this settings window and the menu bar menu.',
  'settings.language.system': 'System',
  'settings.keyboardCustomization': 'Modifier keys and shortcuts',
  'settings.windowManagement': 'Window placement',

  'settings.keepAwakeToggle': 'Prevent Sleep',
  'settings.sessionPageDescription':
    'Keep downloads, builds, and other long jobs running—even when your MacBook lid is closed.',
  'settings.currentSession': 'This session',
  'settings.currentSessionHint': 'This is temporary and always turns off when Tomari quits.',
  'settings.keepAwakeAction': 'Keep this Mac awake',
  'settings.keepAwakeActive': 'Sleep is being prevented',
  'settings.keepAwakeInactive': 'Your Mac can sleep normally',
  'settings.keepAwakeHint':
    'Turning this on or off asks for your administrator password. Expect more battery use and heat while it is on.',
  'settings.lidClose': 'Work with lid closed',
  'settings.lidActive': 'Active',
  'settings.lidPending': 'Enabling…',
  'settings.lidUnavailable': 'Unavailable',
  'settings.lidOff': 'Off',

  'settings.externalControl': 'Links from other apps',
  'settings.externalControlHint':
    'Turn this on only if you use a launcher such as Raycast or Alfred to move windows with tomari:// links.',
  'settings.externalWindowActions': 'Allow tomari:// window commands',

  'settings.maintenance': 'About and updates',
  'settings.maintenanceHint': 'See the installed version or check for a newer release.',
  'settings.version': 'Version',
  'settings.updateAvailable': 'Version {version} is available.',
  'settings.updateFailed': 'Update failed: {error}',
  'settings.installRestart': 'Install and Restart',
  'settings.installing': 'Installing…',
  'settings.upToDate': 'Tomari is up to date.',
  'settings.updateCheckFailed': 'Could not check for updates: {error}',
  'settings.checkUpdates': 'Check for Updates',
  'settings.checking': 'Checking…',
  'settings.saveFailed': 'Could not save settings: {error}',
  'settings.working': 'Working…',

  'settings.applyWarningTitle': 'Saved, but not fully applied',
  'settings.applyWarningShell':
    'macOS could not apply part of the latest change. Review the details and how to retry.',
  'settings.reviewWarning': 'Review',
  'settings.applyWarning.launchAtLogin':
    'Launch at login was saved but could not be applied to the system. Toggle it off and on to try again.',
  'settings.applyWarning.menuBar':
    'The menu bar setting was saved but could not be applied. Toggle it off and on to try again.',
  'settings.applyWarning.keyboardTap':
    'Keyboard customization was saved but its event tap could not start. Check Input Monitoring access, then toggle it off and on.',
  'settings.applyWarning.globalShortcuts':
    'Keyboard customization was saved, but live shortcuts could not be updated. Toggle Keyboard off and on to try again.',
  'settings.applyWarning.dragToSnapTap':
    'Drag to snap was saved but its event tap could not start. Check Input Monitoring access, then toggle it off and on.',
  'settings.applyWarning.dragToMoveTap':
    'Drag to move was saved but its event tap could not start. Check Input Monitoring access, then toggle it off and on.',
  'settings.applyWarning.capsLockRemap':
    'The setting was saved, but the Caps Lock remap could not be updated. Toggle it off and on to try again.',
  'settings.applyWarning.commandImeRules':
    'The Command-key IME switch was saved but could not be applied to the live keyboard. Toggle it off and on to try again.',
  'settings.applyWarning.generic': 'A setting was saved but could not be applied to the system.',
} as const;

export type MessageKey = keyof typeof en;

const ja: Record<MessageKey, string> = {
  'app.nav.keyboard': 'キーボード',
  'app.nav.window': 'ウィンドウ',
  'app.nav.menubar': 'メニューバー',
  'app.nav.session': 'スリープ防止',
  'app.nav.general': '一般',
  'app.sections': 'セクション',
  'app.tools': 'ツール',
  'app.app': 'アプリ',
  'app.permissionsReady': '権限: 準備完了',
  'app.permissionsAttention': '権限: 要確認',

  'common.add': '追加',
  'common.delete': '削除',
  'common.deleteConfirm': '{label} を削除しますか？',
  'common.deleteConfirmShort': '削除する？',
  'common.cancel': 'キャンセル',
  'common.empty': 'まだ何もありません。',
  'common.label': 'ラベル',
  'common.loading': '読み込み中…',
  'common.on': 'オン',
  'common.off': 'オフ',
  'common.enable': '{label} を有効化',
  'common.turnOn': 'オンにする',
  'common.loadFailed': '設定を読み込めませんでした: {error}',
  'common.retry': '再試行',
  'common.refresh': '更新',
  'common.undo': '元に戻す',
  'common.redo': 'やり直す',
  'common.forget': '忘れる',

  'preset.leftHalf': '左半分',
  'preset.rightHalf': '右半分',
  'preset.topHalf': '上半分',
  'preset.bottomHalf': '下半分',
  'preset.topLeftQuarter': '左上',
  'preset.topRightQuarter': '右上',
  'preset.bottomLeftQuarter': '左下',
  'preset.bottomRightQuarter': '右下',
  'preset.leftThird': '左 ⅓',
  'preset.centerThird': '中央 ⅓',
  'preset.rightThird': '右 ⅓',
  'preset.leftTwoThirds': '左 ⅔',
  'preset.rightTwoThirds': '右 ⅔',
  'preset.center': '中央',
  'preset.maximize': '最大化',

  'side.left': '左',
  'side.right': '右',
  'side.either': '左右',

  'direction.next': '次のディスプレイ',
  'direction.prev': '前のディスプレイ',

  'action.togglePanel': 'Tomari の表示切替',
  'action.snap': 'スナップ: {target}',
  'action.moveToDisplay': '{display} へ移動',
  'action.recallPlacement': '保存位置へ戻す',
  'action.moveAndRecall': '{display}へ移動して復元',
  'action.undoWindow': 'ウィンドウ操作を元に戻す',
  'action.redoWindow': 'ウィンドウ操作をやり直す',
  'action.sendKeystroke': '送信: {keys}',
  'action.ime': 'IME → {mode}',
  'action.toggleKeepAwake': 'スリープ防止の切り替え',
  'action.toggleMenuBar': 'メニューバーのアイコン表示切替',
  'action.noOp': '何もしない',

  'keyboard.modifierKeys': '単押しと長押し',
  'keyboard.pageDescription': '単押しの動作を選び、長押しではいつもの修飾キーとして使えます。',
  'keyboard.tabsLabel': 'キーボード設定',
  'keyboard.tab.modifiers': '修飾キー',
  'keyboard.tab.shortcuts': 'ショートカット',
  'keyboard.table.key': 'キー',
  'keyboard.table.tap': '単押し',
  'keyboard.table.hold': '長押し',
  'keyboard.table.enabled': '有効',
  'keyboard.inputSwitching': '入力切り替え',
  'keyboard.leftCommand': '左 Command',
  'keyboard.rightCommand': '右 Command',
  'keyboard.imeEisu': '英数',
  'keyboard.imeKana': 'かな',
  'keyboard.addShortcut': 'ショートカットを追加',
  'keyboard.cancelAddShortcut': 'ショートカットの追加をやめる',
  'keyboard.globalShortcuts': 'Tomari のショートカット',
  'keyboard.usedAs': '{modifier} として使用',
  'keyboard.usedAsHyper': 'Hyper (⌃⌥⇧⌘) として使用',
  'keyboard.tapFor': '押すと {action}',
  'keyboard.tapHold': '押すと {action}、長押しで {modifier}',
  'keyboard.tapAction': '単押し時の操作',
  'keyboard.tapActionFor': '{modifier} を単押ししたときの操作',
  'keyboard.commandImeSwitch': '⌘ で IME 切替',
  'keyboard.commandImeSwitchDesc': '左 ⌘ で英数、右 ⌘ でかな。',
  'keyboard.commandImeSwitchNote':
    '単押し時だけ置き換え、ほかのキーと同時に押した場合は通常の修飾キーとして動作します。',
  'keyboard.shortcutLabelAria': 'ショートカットのラベル',
  'keyboard.actionAria': 'アクション',
  'keyboard.recordShortcut': 'ショートカットを記録',
  'keyboard.changeShortcut': '{label} のショートカットを変更',
  'keyboard.deleteShortcut': '{label} を削除',
  'keyboard.offNote': 'キーボードカスタマイズはオフです。タップ・ショートカットは実行されません。',
  'keyboard.imNeeded': '入力監視(Input Monitoring)へのアクセスが必要です',
  'keyboard.imBody': '押す/長押し・Hyper キーの検出に必要です。',
  'keyboard.noModifierRules': '設定できる修飾キーがありません。',
  'keyboard.noHotkeys':
    'ショートカットはまだありません。「ショートカットを追加」から作成できます。',

  'recorder.click': 'クリックして記録',
  'recorder.typing': 'ショートカットを入力…',
  'recorder.needModifier': '修飾キーを追加してください',
  'recorder.unsupported': 'このショートカットは使えません',
  'recorder.startFailed': '記録を開始できませんでした',

  'error.permissionRequired': 'アクセシビリティの許可が必要です。',
  'error.noFocusedWindow': '操作対象のウィンドウがありません。',
  'error.shortcutConflict':
    'このショートカットを登録できませんでした。他のアプリと競合している可能性があります。',
  'error.placementNotFound': '先にこのアプリの位置を記憶してください。',
  'error.windowTargetChanged': '操作対象のウィンドウが変わったため、表示を更新しました。',
  'error.windowNotResponding':
    '対象アプリがウィンドウ操作に応答しませんでした。少し待ってから更新してください。',

  'setup.title': 'Tomari を使う準備',
  'setup.intro':
    'Tomari がウィンドウを動かし、キー操作を認識するために macOS の権限を 2 つ許可します。権限はあとから見直せます。',
  'setup.accessibility': 'アクセシビリティ',
  'setup.accessibilityWhy':
    'ウィンドウの移動・リサイズ、キー送信、メニューバー配置の読み取りに使います。',
  'setup.inputMonitoring': '入力監視',
  'setup.inputMonitoringWhy':
    '修飾キーの単押し・長押し、Hyper キー、ウィンドウのドラッグ操作を認識するために使います。',
  'setup.granted': '付与済み',
  'setup.grant': 'システム設定を開く',
  'setup.grantFor': '{name} のシステム設定を開く',
  'setup.requesting': '開いています…',
  'setup.openAgain': 'もう一度開く',
  'setup.returnHint': 'システム設定で許可したら、Tomari に戻ってください。',
  'setup.later': 'あとで設定する',
  'setup.done': 'Tomari を使い始める',
  'setup.allSet': '準備完了です。Tomari を使い始められます。',
  'setup.tryIt': '試してみる: {keys} で直前まで使っていたウィンドウが左半分にスナップします',
  'setup.bannerText.one': 'macOS の権限があと 1 つ必要です',
  'setup.bannerText.two': 'macOS の権限が 2 つ必要です',
  'setup.updateBannerText': 'アップデート後の権限を確認してください',
  'setup.bannerDescription.both': '許可するまでは、ウィンドウ操作とキー操作の一部が制限されます。',
  'setup.bannerDescription.accessibility':
    '許可するまでは、ウィンドウ操作、キー送信、メニューバーの読み取りが制限されます。',
  'setup.bannerDescription.inputMonitoring':
    '許可するまでは、修飾キー操作とウィンドウのドラッグ機能が制限されます。',
  'setup.bannerAction': '権限を確認',
  'setup.openSetup': 'セットアップを開く',
  'setup.updateRegrant':
    'アップデート後に権限が外れてしまったようです（現時点の Tomari の既知の制約です）。もう一度許可してください。',
  'setup.adminNote':
    'スリープ防止は権限とは別で、使うたびに管理者パスワードを確認します。ここでの許可は不要です。',

  'window.axNeeded': 'アクセシビリティへのアクセスが必要です',
  'window.pageDescription':
    '保存位置と、ウィンドウを動かすショートカット・マウス操作を設定します。',
  'window.tabsLabel': 'ウィンドウ設定',
  'window.tab.saved': '保存位置',
  'window.tab.shortcuts': 'ショートカット',
  'window.tab.mouse': 'マウス',
  'window.currentWindow': '現在のウィンドウ',
  'window.basicShortcuts': 'よく使うショートカット',
  'window.moreShortcuts': 'その他のレイアウト {count} 件',
  'window.showMoreShortcuts': 'その他のレイアウトを表示',
  'window.hideMoreShortcuts': 'その他のレイアウトを閉じる',
  'window.addShortcut': 'ショートカットを追加',
  'window.dragGesture': '画面端へドラッグ',
  'window.resizeGesture': '修飾キーで移動・リサイズ',
  'window.axBody': '他のアプリのウィンドウの移動・リサイズに必要です。',
  'window.grantAccess': 'システム設定を開く',
  'window.offNote': 'ウィンドウ管理はオフです。スナップ・ディスプレイ移動は動作しません。',
  'window.focusedApp': '操作対象のアプリ',
  'window.noFocusedApp': 'ウィンドウを選んで更新してください',
  'window.restoreHome': '保存位置へ戻す',
  'window.moveAndRestore': '次の画面へ移動して復元',
  'window.restoredSlot': '{app} を {slot} へ戻しました',
  'window.movedAndRestoredSlot': '{app} を移動して {slot} へ戻しました',
  'window.noAdjacentDisplay': '移動先のディスプレイがありません',
  'window.undone': 'ウィンドウ操作を元に戻しました',
  'window.redone': 'ウィンドウ操作をやり直しました',
  'window.historyEmpty': '適用できるウィンドウ操作はありません',
  'window.staleHistoryDiscarded': '閉じたウィンドウの履歴を破棄しました',
  'window.rememberedHomes': 'このアプリの保存位置',
  'window.slot.primary': '保存位置 A',
  'window.slot.secondary': '保存位置 B',
  'window.rememberedCount': '記憶済みの位置: {count} 件',
  'window.previewAria':
    '{app} のウィンドウ位置。記憶済みは {count} 件です。アンバーの実線が現在位置、輪郭線が記憶した位置です。',
  'window.previewEmptyAria': '表示できるウィンドウ位置がありません',
  'window.currentPosition': '現在位置',
  'window.homeReady': 'どのディスプレイでも使えます',
  'window.homeEmpty': '位置はまだ記憶されていません',
  'window.lastRestored': '最後に復元',
  'window.rememberHere': '現在位置を保存',
  'window.replaceHome': '保存位置を置き換える',
  'window.remembered': '{slot} を記憶しました',
  'window.alreadyRemembered': '{slot} はすでに現在位置と同じです',
  'window.forgotten': '{slot} を削除しました',
  'window.savedEditUndone': '記憶した位置を元に戻しました',
  'window.noSavedEditToUndo': '元に戻せる位置の編集はありません',
  'window.confirmForget': '削除する？',
  'window.confirmForgetAria': '{slot} を削除しますか？',
  'window.forgetAria': '{slot} を削除',
  'window.controls': '操作方法',
  'window.modifierTapActions': '修飾キー単押しの操作',
  'window.modifierTapActionsHint':
    '必要ならキーボード設定で Caps Lock などの単押しに「位置を復元」を割り当てられます。',
  'window.modifierTapActionsDisabled':
    'これらのショートカットを使い、修飾キーの単押しに復元を割り当てるには、キーボードカスタマイズを有効にしてください。',
  'window.openKeyboard': 'キーボード設定を開く',
  'window.noShortcuts':
    'ウィンドウ用ショートカットはまだありません。「ショートカットを追加」から作成できます。',
  'window.mouse': 'マウス',
  'window.mouseDescription':
    '画面端へのドラッグや修飾キーを押しながらのドラッグで、ウィンドウを動かすか選べます。',
  'window.dragToSnapToggle': '画面端へのドラッグでスナップ',
  'window.enableDragToSnap': 'ドラッグスナップを有効化',
  'window.dragToSnapHint': '画面の端や隅へドラッグし、プレビューが出たら離します。',

  'window.dragToMoveToggle': 'ポインタの下のウィンドウを移動・リサイズ',
  'window.enableDragToMove': 'ドラッグで移動・リサイズを有効化',
  'window.dragToMoveHint': '⌃⌥ を押しながらドラッグして移動します。⌘ を加えるとリサイズします。',

  'menubar.title': 'メニューバー',
  'menubar.pageDescription': '表示する項目と、隠す領域の開閉方法を設定します。',
  'menubar.tabsLabel': 'メニューバー設定',
  'menubar.tab.items': '項目',
  'menubar.tab.behavior': '動作',
  'menubar.diagramLabel': '現在のメニューバー配置',
  'menubar.arrangeInstruction': '下のボタンで、項目を Tomari の区切りの反対側へ移動します。',
  'menubar.offNote': 'メニューバー整理はオフです。区切りは表示されません。',
  'menubar.enable': 'メニューバー整理を有効化',
  'menubar.showToggle': '隠したアイコンを表示',
  'menubar.showDesc': 'メニューバーの ‹、トレイ、ショートカットからも操作できます。',
  'menubar.iconsHidden': '隠したアイコンは折りたたまれています',
  'menubar.iconsVisible': '隠したアイコンを表示しています',
  'menubar.showAction': 'アイコンを表示',
  'menubar.hideAction': 'もう一度隠す',
  'menubar.autoCollapse': '自動で折りたたむ',
  'menubar.autoCollapseNever': 'しない',
  'menubar.autoCollapseSecs': '{secs} 秒後',
  'menubar.behaviorSection': '開閉のしかた',
  'menubar.arrangeSection': '隠すものを決める',
  'menubar.arrangeBody':
    '⌘ を押しながらメニューバーのアイコンをドラッグして、隠したいものを ≡ より左に並べます。',
  'menubar.inventoryLoading': 'メニューバー項目を読み取っています…',
  'menubar.inventoryEmpty': 'この領域に項目は検出されませんでした。',
  'menubar.inventoryError':
    'メニューバーを読み取れませんでした。展開後にもう一度更新してください。',
  'menubar.inventoryUnsupported': 'メニューバーの読み取りは macOS で利用できます。',
  'menubar.inventoryDividerMissing':
    '区切りを取得できません。メニューバー整理をオフにしてから再度オンにし、更新してください。',
  'menubar.inventoryPermission': 'メニューバー項目の特定にはアクセシビリティ権限が必要です。',
  'menubar.grantAccessibility': 'アクセスを許可…',
  'menubar.refreshItems': '項目を更新',
  'menubar.hiddenItems': '現在隠れる項目',
  'menubar.visibleItems': '常に表示する項目',
  'menubar.itemCount': '{count}',
  'menubar.itemOwner': '{owner} から',
  'menubar.zoneHidden': '隠れる',
  'menubar.zoneVisible': '常に表示',
  'menubar.moveHide': '隠す',
  'menubar.moveShow': '常に表示',
  'menubar.moveHideItem': '{item} を隠す',
  'menubar.moveShowItem': '{item} を常に表示',
  'menubar.moving': '移動中…',
  'menubar.movingItem': '{item} を移動中…',
  'menubar.moveStale':
    '{item} の移動中にメニューバーが変わりました。一覧を更新したので、もう一度お試しください。',
  'menubar.moveFailed': '{item} を移動できませんでした。',
  'menubar.moveManualFallback':
    '代わりに、実際のメニューバーで ⌘ を押しながら区切りの反対側へドラッグしてください。',

  'settings.general': '一般',
  'settings.pageDescription':
    'Tomari をいつ起動するか、どこに表示するか、ほかのアプリからの操作を許可するかを選びます。',
  'settings.startup': '起動と表示',
  'settings.startupDescription': 'Tomari の開き方と表示言語を設定します。',
  'settings.launchAtLogin': 'ログイン時に起動',
  'settings.launchAtLoginHint': 'Mac へのログイン時に Tomari を自動で起動します。',
  'settings.showInMenuBar': 'メニューバーに表示',
  'settings.showInMenuBarHint': 'Tomari のアイコンを表示し、すぐ開けるようにします。',
  'settings.hiddenHint':
    'メニューバー非表示中でも、Spotlight や Launchpad から Tomari を再度起動すればいつでも開けます。グローバルショートカット（デフォルト ⌘⇧K）でも開けます。',
  'settings.hideTrayConfirmTitle': 'メニューバーアイコンを非表示にしますか？',
  'settings.hideTrayConfirmBody':
    'Tomari はメニューバーにも Dock にもアイコンを出さずに動き続けます。再び開くには、Spotlight や Launchpad から Tomari を起動するか、グローバルショートカット（デフォルト ⌘⇧K）を使ってください。',
  'settings.hideTrayConfirmAction': 'アイコンを非表示',
  'settings.language': '言語',
  'settings.languageHint': 'この設定画面とメニューバーメニューの表示言語を変えます。',
  'settings.language.system': 'システム',
  'settings.keyboardCustomization': '修飾キーとショートカット',
  'settings.windowManagement': 'ウィンドウの位置と移動',

  'settings.keepAwakeToggle': 'スリープ防止',
  'settings.sessionPageDescription':
    'ダウンロードやビルドなどの長い処理を、MacBook の蓋を閉じても続けます。',
  'settings.currentSession': '今回のセッション',
  'settings.currentSessionHint': '一時的な機能です。Tomari を終了すると必ず解除されます。',
  'settings.keepAwakeAction': 'この Mac のスリープを防ぐ',
  'settings.keepAwakeActive': '現在、スリープを防いでいます',
  'settings.keepAwakeInactive': '現在、通常どおりスリープします',
  'settings.keepAwakeHint':
    'オン・オフの切り替え時に管理者パスワードを確認します。オンの間はバッテリー消費と発熱が増えます。',
  'settings.lidClose': '蓋を閉じても継続',
  'settings.lidActive': '有効',
  'settings.lidPending': '有効化中…',
  'settings.lidUnavailable': '利用不可',
  'settings.lidOff': 'オフ',

  'settings.externalControl': 'ほかのアプリとの連携',
  'settings.externalControlHint':
    'Raycast や Alfred などから tomari:// リンクでウィンドウを動かす場合だけオンにしてください。',
  'settings.externalWindowActions': 'tomari:// のウィンドウ操作を許可',

  'settings.maintenance': 'このアプリとアップデート',
  'settings.maintenanceHint': '現在のバージョンを確認し、新しいリリースを探します。',
  'settings.version': 'バージョン',
  'settings.updateAvailable': 'バージョン {version} が利用可能です。',
  'settings.updateFailed': 'アップデートに失敗しました: {error}',
  'settings.installRestart': 'インストールして再起動',
  'settings.installing': 'インストール中…',
  'settings.upToDate': 'Tomari は最新です。',
  'settings.updateCheckFailed': 'アップデートを確認できませんでした: {error}',
  'settings.checkUpdates': 'アップデートを確認',
  'settings.checking': '確認中…',
  'settings.saveFailed': '設定を保存できませんでした: {error}',
  'settings.working': '処理中…',

  'settings.applyWarningTitle': '保存しましたが、適用しきれませんでした',
  'settings.applyWarningShell':
    '直前の変更の一部を macOS に適用できませんでした。詳細と再試行方法を確認してください。',
  'settings.reviewWarning': '確認する',
  'settings.applyWarning.launchAtLogin':
    'ログイン時に起動の設定は保存しましたが、システムに適用できませんでした。オフにしてからもう一度オンにすると再試行します。',
  'settings.applyWarning.menuBar':
    'メニューバーの設定は保存しましたが、適用できませんでした。オフにしてからもう一度オンにすると再試行します。',
  'settings.applyWarning.keyboardTap':
    'キーボードカスタマイズは保存しましたが、イベントタップを開始できませんでした。入力監視の許可を確認して、オフにしてからもう一度オンにしてください。',
  'settings.applyWarning.globalShortcuts':
    'キーボードカスタマイズは保存しましたが、実行中のショートカットを更新できませんでした。キーボードをオフにしてからもう一度オンにしてください。',
  'settings.applyWarning.dragToSnapTap':
    'ドラッグでスナップは保存しましたが、イベントタップを開始できませんでした。入力監視の許可を確認して、オフにしてからもう一度オンにしてください。',
  'settings.applyWarning.dragToMoveTap':
    'ドラッグで移動は保存しましたが、イベントタップを開始できませんでした。入力監視の許可を確認して、オフにしてからもう一度オンにしてください。',
  'settings.applyWarning.capsLockRemap':
    '設定は保存しましたが、Caps Lock の割り当てを更新できませんでした。オフにしてからもう一度オンにすると再試行します。',
  'settings.applyWarning.commandImeRules':
    'Command キーでの IME 切り替えは保存しましたが、動作中のキーボードに適用できませんでした。オフにしてからもう一度オンにすると再試行します。',
  'settings.applyWarning.generic': '設定は保存しましたが、システムに適用できませんでした。',
};

export type Lang = 'en' | 'ja';

export const DICTS: Record<Lang, Record<MessageKey, string>> = { en, ja };

/** Resolve the language setting to a concrete UI language. */
export function resolveLang(language: Language): Lang {
  if (language === 'system') {
    return navigator.language.toLowerCase().startsWith('ja') ? 'ja' : 'en';
  }
  return language;
}

export function translate(
  lang: Lang,
  key: MessageKey,
  params?: Record<string, string | number>,
): string {
  let message: string = DICTS[lang][key];
  if (params) {
    for (const [name, value] of Object.entries(params)) {
      message = message.replaceAll(`{${name}}`, String(value));
    }
  }
  return message;
}

export type Translator = (key: MessageKey, params?: Record<string, string | number>) => string;

const I18nContext = createContext<Lang>('en');

export function I18nProvider({ lang, children }: { lang: Lang; children: ReactNode }) {
  return <I18nContext.Provider value={lang}>{children}</I18nContext.Provider>;
}

/** The translate function for the current UI language. */
export function useT(): Translator {
  const lang = useContext(I18nContext);
  return (key, params) => translate(lang, key, params);
}
