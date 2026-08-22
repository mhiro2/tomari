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
  'app.featureOff': 'Off',

  'common.add': 'Add',
  'common.delete': 'Delete',
  'common.deleteConfirm': 'Delete {label}?',
  'common.deleteConfirmShort': 'Delete?',
  'common.cancel': 'Cancel',
  'common.empty': 'Nothing here yet.',
  'common.label': 'Label',
  'common.loading': 'Loading…',
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
  'action.recallPlacement': 'Restore Remembered Position',
  'action.moveAndRecall': 'Move & Restore on {display}',
  'action.undoWindow': 'Undo Window Change',
  'action.redoWindow': 'Redo Window Change',
  'action.sendKeystroke': 'Send: {keys}',
  'action.ime': 'IME → {mode}',
  'action.toggleKeepAwake': 'Toggle Prevent Sleep',
  'action.toggleMenuBar': 'Show/Hide Menu Bar Icons',
  'action.noOp': 'Do Nothing',

  'keyboard.modifierKeys': 'Modifier keys',
  'keyboard.modifierHint':
    'Tap a modifier alone to fire its command; hold it and system shortcuts keep working.',
  'keyboard.globalShortcuts': 'Global shortcuts',
  'keyboard.usedAs': 'Used as {modifier}',
  'keyboard.usedAsHyper': 'Used as Hyper (⌃⌥⇧⌘)',
  'keyboard.tapFor': 'Tap for {action}',
  'keyboard.tapHold': 'Tap for {action}, hold for {modifier}',
  'keyboard.tapAction': 'Tap action',
  'keyboard.tapActionFor': 'Tap action for {modifier}',
  'keyboard.commandImeSwitch': 'Command-key IME switch',
  'keyboard.commandImeSwitchDesc': 'Tap left ⌘ for 英数, right ⌘ for かな.',
  'keyboard.shortcutLabelAria': 'Shortcut label',
  'keyboard.actionAria': 'Action',
  'keyboard.recordShortcut': 'Record Shortcut',
  'keyboard.changeShortcut': 'Change shortcut for {label}',
  'keyboard.deleteShortcut': 'Delete {label}',
  'keyboard.offNote': "Keyboard customization is off — taps and shortcuts won't fire.",
  'keyboard.imNeeded': 'Input Monitoring access needed',
  'keyboard.imBody': 'Needed to detect taps, holds, and the hyper key.',
  'keyboard.noModifierRules': 'No modifier keys to configure.',
  'keyboard.noHotkeys': 'No global shortcuts yet — add one below.',

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

  'setup.title': 'Set up Tomari',
  'setup.intro':
    'Tomari needs two macOS permissions to move windows and customize your keyboard. Grant them here to get started.',
  'setup.accessibility': 'Accessibility',
  'setup.accessibilityWhy': 'Needed to move windows and send keys.',
  'setup.inputMonitoring': 'Input Monitoring',
  'setup.inputMonitoringWhy':
    'Needed to tap/hold and remap modifier keys, and for the drag features.',
  'setup.granted': 'Granted',
  'setup.grant': 'Grant Access',
  'setup.grantFor': 'Grant Access for {name}',
  'setup.later': 'Set up later',
  'setup.done': 'Done',
  'setup.allSet': 'All set — Tomari is ready to use.',
  'setup.tryIt': 'Try it: press {keys} to snap the window you were just using to the left half.',
  'setup.bannerText': "Setup isn't finished yet.",
  'setup.bannerAction': 'Continue',
  'setup.openSetup': 'Open Setup',
  'setup.updateRegrant':
    "Tomari's permissions went missing after the update — a known limitation for now. Please grant them again.",
  'setup.adminNote':
    'Prevent Sleep is separate: it asks for your administrator password each time instead of a one-time grant.',

  'window.axNeeded': 'Accessibility access needed',
  'window.axBody': 'Needed to move and resize other apps’ windows.',
  'window.grantAccess': 'Grant Access',
  'window.offNote': "Window management is off — snapping and display moves won't work.",
  'window.focusedApp': 'Focused app',
  'window.noFocusedApp': 'Choose a window, then refresh',
  'window.restoreHome': 'Restore position',
  'window.moveAndRestore': 'Next display & restore',
  'window.restoredSlot': 'Restored {app} to {slot}',
  'window.movedAndRestoredSlot': 'Moved {app} and restored {slot}',
  'window.noAdjacentDisplay': 'No other display is available',
  'window.undone': 'Window change undone',
  'window.redone': 'Window change redone',
  'window.historyEmpty': 'There is no window change to apply',
  'window.staleHistoryDiscarded': 'Closed-window history was discarded',
  'window.rememberedHomes': 'Remembered positions',
  'window.rememberedHomesHint':
    'Positions are relative to the current display, so they adapt safely to a different screen size.',
  'window.slot.primary': 'Home 1',
  'window.slot.secondary': 'Home 2',
  'window.rememberedCount': 'Remembered positions: {count}',
  'window.previewAria':
    '{app} window position preview. Remembered position count: {count}. Solid amber is the current position; outlined frames are remembered positions.',
  'window.previewEmptyAria': 'No focused window position to preview',
  'window.currentPosition': 'Current',
  'window.homeReady': 'Ready on any display',
  'window.homeEmpty': 'No position remembered',
  'window.lastRestored': 'Last restored',
  'window.rememberHere': 'Remember here',
  'window.replaceHome': 'Replace',
  'window.remembered': 'Remembered {slot}',
  'window.alreadyRemembered': '{slot} already matches this position',
  'window.forgotten': 'Forgot {slot}',
  'window.savedEditUndone': 'Remembered position restored',
  'window.noSavedEditToUndo': 'There is no remembered-position edit to undo',
  'window.confirmForget': 'Forget?',
  'window.confirmForgetAria': 'Forget {slot}?',
  'window.forgetAria': 'Forget {slot}',
  'window.controls': 'Controls',
  'window.controlsHint':
    'Window shortcuts live here. Repeating Restore alternates Home 1 and Home 2.',
  'window.modifierTapActions': 'Modifier-key tap actions',
  'window.modifierTapActionsHint':
    'Optionally assign Restore to a Caps Lock or other modifier-key tap in Keyboard.',
  'window.modifierTapActionsDisabled':
    'Turn on Keyboard customization before assigning Restore to a modifier-key tap.',
  'window.openKeyboard': 'Open Keyboard',
  'window.noShortcuts': 'No window shortcuts yet — add one below.',
  'window.mouse': 'Mouse',
  'window.dragToSnapToggle': 'Snap by dragging to a screen edge',
  'window.enableDragToSnap': 'Enable Drag to Snap',
  'window.dragToSnapHint':
    'Drag a window to a screen edge or corner — a preview appears and the window snaps to a half, corner, or full screen when you let go. Requires Accessibility and Input Monitoring.',

  'window.dragToMoveToggle': 'Move or resize the window under the pointer',
  'window.enableDragToMove': 'Enable Drag to Move & Resize',
  'window.dragToMoveHint':
    'Hold ⌃⌥ and drag anywhere in a window to move it, or ⌃⌥⌘ to resize it from the bottom-right (top-left stays put). Works on the window under the pointer — no need to click first. Requires Accessibility and Input Monitoring.',

  'menubar.title': 'Menu bar tidying',
  'menubar.offNote': 'Menu bar tidying is off — Tomari adds no divider to your menu bar.',
  'menubar.enable': 'Turn on menu bar tidying',
  'menubar.showToggle': 'Show hidden icons',
  'menubar.showDesc': 'Also from the ‹ item, the tray menu, or a shortcut.',
  'menubar.autoCollapse': 'Collapse automatically',
  'menubar.autoCollapseNever': 'Never',
  'menubar.autoCollapseSecs': 'After {secs} seconds',
  'menubar.arrangeSection': 'Choosing what to hide',
  'menubar.arrangeBody':
    'Hold ⌘ and drag your menu bar icons so the ones you want tucked away sit left of the ≡ divider.',
  'menubar.inventoryIntro':
    'This is the current menu bar arrangement. Move an icon across the ≡ divider with ⌘-drag, then refresh.',
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
  'menubar.limitNote':
    'macOS only lets an app move its own menu bar icons. Expanding may not reveal everything if the frontmost app has a long menu bar, or your Mac has a notch.',

  'settings.general': 'General',
  'settings.launchAtLogin': 'Launch at login',
  'settings.showInMenuBar': 'Show in menu bar',
  'settings.hiddenHint':
    'Hidden from the menu bar — reopen Tomari any time by launching it again from Spotlight or Launchpad, or with the global shortcut (default ⌘⇧K).',
  'settings.hideTrayConfirmTitle': 'Hide the menu bar icon?',
  'settings.hideTrayConfirmBody':
    'Tomari keeps running with no menu bar icon and no Dock icon. To open it again, launch Tomari from Spotlight or Launchpad, or use the global shortcut (default ⌘⇧K).',
  'settings.hideTrayConfirmAction': 'Hide Icon',
  'settings.language': 'Language',
  'settings.language.system': 'System',
  'settings.keyboardCustomization': 'Keyboard customization',
  'settings.windowManagement': 'Window management',

  'settings.keepAwakeToggle': 'Prevent Sleep',
  'settings.keepAwakeHint':
    "Keeps your Mac — and anything it's running — awake even with the lid closed. Asks for your administrator password when turning it on. Expect more battery use and heat.",
  'settings.lidClose': 'Work with lid closed',
  'settings.lidActive': 'Active',
  'settings.lidPending': 'Enabling…',
  'settings.lidUnavailable': 'Unavailable',
  'settings.lidOff': 'Off',

  'settings.externalControl': 'External control',
  'settings.externalControlHint':
    "Let launchers like Raycast and Alfred place the focused window through the tomari:// URL scheme. This is a security boundary — leave it off if you don't use it.",
  'settings.externalWindowActions': 'URL scheme control',

  'settings.maintenance': 'Maintenance',
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
  'settings.applyWarning.launchAtLogin':
    'Launch at login was saved but could not be applied to the system. Toggle it off and on to try again.',
  'settings.applyWarning.menuBar':
    'The menu bar setting was saved but could not be applied. Toggle it off and on to try again.',
  'settings.applyWarning.keyboardTap':
    'Keyboard customization was saved but its event tap could not start. Check Input Monitoring access, then toggle it off and on.',
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
  'app.featureOff': 'オフ',

  'common.add': '追加',
  'common.delete': '削除',
  'common.deleteConfirm': '{label} を削除しますか？',
  'common.deleteConfirmShort': '削除する？',
  'common.cancel': 'キャンセル',
  'common.empty': 'まだ何もありません。',
  'common.label': 'ラベル',
  'common.loading': '読み込み中…',
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
  'action.recallPlacement': '記憶した位置へ戻す',
  'action.moveAndRecall': '{display}へ移動して復元',
  'action.undoWindow': 'ウィンドウ操作を元に戻す',
  'action.redoWindow': 'ウィンドウ操作をやり直す',
  'action.sendKeystroke': '送信: {keys}',
  'action.ime': 'IME → {mode}',
  'action.toggleKeepAwake': 'スリープ防止の切り替え',
  'action.toggleMenuBar': 'メニューバーのアイコン表示切替',
  'action.noOp': '何もしない',

  'keyboard.modifierKeys': '修飾キー',
  'keyboard.modifierHint':
    '修飾キーを単独で押すとコマンドを実行します。長押しすると通常の修飾キーとして使えます。',
  'keyboard.globalShortcuts': 'グローバルショートカット',
  'keyboard.usedAs': '{modifier} として使用',
  'keyboard.usedAsHyper': 'Hyper (⌃⌥⇧⌘) として使用',
  'keyboard.tapFor': '押すと {action}',
  'keyboard.tapHold': '押すと {action}、長押しで {modifier}',
  'keyboard.tapAction': '単押し時の操作',
  'keyboard.tapActionFor': '{modifier} を単押ししたときの操作',
  'keyboard.commandImeSwitch': '⌘ で IME 切替',
  'keyboard.commandImeSwitchDesc': '左 ⌘ で英数、右 ⌘ でかな。',
  'keyboard.shortcutLabelAria': 'ショートカットのラベル',
  'keyboard.actionAria': 'アクション',
  'keyboard.recordShortcut': 'ショートカットを記録',
  'keyboard.changeShortcut': '{label} のショートカットを変更',
  'keyboard.deleteShortcut': '{label} を削除',
  'keyboard.offNote': 'キーボードカスタマイズはオフです。タップ・ショートカットは実行されません。',
  'keyboard.imNeeded': '入力監視(Input Monitoring)へのアクセスが必要です',
  'keyboard.imBody': '押す/長押し・Hyper キーの検出に必要です。',
  'keyboard.noModifierRules': '設定できる修飾キーがありません。',
  'keyboard.noHotkeys': 'グローバルショートカットはまだありません。下から追加してください。',

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

  'setup.title': 'Tomari をセットアップ',
  'setup.intro':
    'ウィンドウ操作とキーボードカスタマイズには macOS の権限が 2 つ必要です。ここから許可して始めましょう。',
  'setup.accessibility': 'アクセシビリティ',
  'setup.accessibilityWhy': 'ウィンドウの移動やキー送信に必要です。',
  'setup.inputMonitoring': '入力監視',
  'setup.inputMonitoringWhy': '修飾キーの押す/長押しやリマップ、ドラッグ機能に必要です。',
  'setup.granted': '付与済み',
  'setup.grant': '許可する',
  'setup.grantFor': '{name} を許可する',
  'setup.later': 'あとで設定する',
  'setup.done': '完了',
  'setup.allSet': '準備完了です。Tomari を使い始められます。',
  'setup.tryIt': '試してみる: {keys} で直前まで使っていたウィンドウが左半分にスナップします',
  'setup.bannerText': 'セットアップが完了していません',
  'setup.bannerAction': '続ける',
  'setup.openSetup': 'セットアップを開く',
  'setup.updateRegrant':
    'アップデート後に権限が外れてしまったようです（現時点の Tomari の既知の制約です）。もう一度許可してください。',
  'setup.adminNote':
    'スリープ防止は権限とは別で、使うたびに管理者パスワードを確認します。ここでの許可は不要です。',

  'window.axNeeded': 'アクセシビリティへのアクセスが必要です',
  'window.axBody': '他のアプリのウィンドウの移動・リサイズに必要です。',
  'window.grantAccess': 'アクセスを許可',
  'window.offNote': 'ウィンドウ管理はオフです。スナップ・ディスプレイ移動は動作しません。',
  'window.focusedApp': '操作対象のアプリ',
  'window.noFocusedApp': 'ウィンドウを選んで更新してください',
  'window.restoreHome': '記憶した位置へ戻す',
  'window.moveAndRestore': '次の画面へ移動して復元',
  'window.restoredSlot': '{app} を {slot} へ戻しました',
  'window.movedAndRestoredSlot': '{app} を移動して {slot} へ戻しました',
  'window.noAdjacentDisplay': '移動先のディスプレイがありません',
  'window.undone': 'ウィンドウ操作を元に戻しました',
  'window.redone': 'ウィンドウ操作をやり直しました',
  'window.historyEmpty': '適用できるウィンドウ操作はありません',
  'window.staleHistoryDiscarded': '閉じたウィンドウの履歴を破棄しました',
  'window.rememberedHomes': '記憶した位置',
  'window.rememberedHomesHint':
    '現在のディスプレイに対する相対位置として保存するため、画面サイズが変わっても安全に合わせます。',
  'window.slot.primary': '位置 1',
  'window.slot.secondary': '位置 2',
  'window.rememberedCount': '記憶済みの位置: {count} 件',
  'window.previewAria':
    '{app} のウィンドウ位置。記憶済みは {count} 件です。アンバーの実線が現在位置、輪郭線が記憶した位置です。',
  'window.previewEmptyAria': '表示できるウィンドウ位置がありません',
  'window.currentPosition': '現在位置',
  'window.homeReady': 'どのディスプレイでも復元できます',
  'window.homeEmpty': '位置はまだ記憶されていません',
  'window.lastRestored': '最後に復元',
  'window.rememberHere': '現在位置を記憶',
  'window.replaceHome': '置き換える',
  'window.remembered': '{slot} を記憶しました',
  'window.alreadyRemembered': '{slot} はすでに現在位置と同じです',
  'window.forgotten': '{slot} を削除しました',
  'window.savedEditUndone': '記憶した位置を元に戻しました',
  'window.noSavedEditToUndo': '元に戻せる位置の編集はありません',
  'window.confirmForget': '削除する？',
  'window.confirmForgetAria': '{slot} を削除しますか？',
  'window.forgetAria': '{slot} を削除',
  'window.controls': '操作方法',
  'window.controlsHint':
    'ウィンドウ用ショートカットはここで管理します。復元を繰り返すと位置 1 / 位置 2 が切り替わります。',
  'window.modifierTapActions': '修飾キー単押しの操作',
  'window.modifierTapActionsHint':
    '必要ならキーボード設定で Caps Lock などの単押しに「位置を復元」を割り当てられます。',
  'window.modifierTapActionsDisabled':
    '修飾キーの単押しに復元を割り当てるには、キーボードカスタマイズを有効にしてください。',
  'window.openKeyboard': 'キーボード設定を開く',
  'window.noShortcuts': 'ウィンドウ用ショートカットはまだありません。下から追加できます。',
  'window.mouse': 'マウス',
  'window.dragToSnapToggle': '画面端へのドラッグでスナップ',
  'window.enableDragToSnap': 'ドラッグスナップを有効化',
  'window.dragToSnapHint':
    'ウィンドウを画面の端や隅にドラッグすると、プレビューが表示され、離した位置に応じて左右半分・四隅・全画面にスナップします。アクセシビリティと入力監視の権限が必要です。',

  'window.dragToMoveToggle': 'ポインタの下のウィンドウを移動・リサイズ',
  'window.enableDragToMove': 'ドラッグで移動・リサイズを有効化',
  'window.dragToMoveHint':
    '⌃⌥ を押しながらウィンドウのどこでもドラッグすると移動、⌃⌥⌘ なら右下方向にリサイズします（左上は固定）。ポインタの下のウィンドウに効くので、先にクリックする必要はありません。アクセシビリティと入力監視の権限が必要です。',

  'menubar.title': 'メニューバー整理',
  'menubar.offNote': 'メニューバー整理はオフです。区切りは表示されません。',
  'menubar.enable': 'メニューバー整理を有効化',
  'menubar.showToggle': '隠したアイコンを表示',
  'menubar.showDesc': 'メニューバーの ‹、トレイ、ショートカットからも操作できます。',
  'menubar.autoCollapse': '自動で折りたたむ',
  'menubar.autoCollapseNever': 'しない',
  'menubar.autoCollapseSecs': '{secs} 秒後',
  'menubar.arrangeSection': '隠すものを決める',
  'menubar.arrangeBody':
    '⌘ を押しながらメニューバーのアイコンをドラッグして、隠したいものを ≡ より左に並べます。',
  'menubar.inventoryIntro':
    '現在のメニューバー配置です。⌘ ドラッグでアイコンを ≡ の反対側へ移動し、更新してください。',
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
  'menubar.limitNote':
    'macOS では、アプリは自分のアイコンしか動かせません。前面アプリのメニューが長いときやノッチのある Mac では、展開しても全部は見えないことがあります。',

  'settings.general': '一般',
  'settings.launchAtLogin': 'ログイン時に起動',
  'settings.showInMenuBar': 'メニューバーに表示',
  'settings.hiddenHint':
    'メニューバー非表示中でも、Spotlight や Launchpad から Tomari を再度起動すればいつでも開けます。グローバルショートカット（デフォルト ⌘⇧K）でも開けます。',
  'settings.hideTrayConfirmTitle': 'メニューバーアイコンを非表示にしますか？',
  'settings.hideTrayConfirmBody':
    'Tomari はメニューバーにも Dock にもアイコンを出さずに動き続けます。再び開くには、Spotlight や Launchpad から Tomari を起動するか、グローバルショートカット（デフォルト ⌘⇧K）を使ってください。',
  'settings.hideTrayConfirmAction': 'アイコンを非表示',
  'settings.language': '言語',
  'settings.language.system': 'システム',
  'settings.keyboardCustomization': 'キーボードカスタマイズ',
  'settings.windowManagement': 'ウィンドウ管理',

  'settings.keepAwakeToggle': 'スリープ防止',
  'settings.keepAwakeHint':
    'ディスプレイを閉じても、Mac と実行中の処理をスリープさせません。オンにするとき管理者パスワードを尋ねます。バッテリー消費と発熱が増える点に注意してください。',
  'settings.lidClose': '蓋を閉じても継続',
  'settings.lidActive': '有効',
  'settings.lidPending': '有効化中…',
  'settings.lidUnavailable': '利用不可',
  'settings.lidOff': 'オフ',

  'settings.externalControl': '外部制御',
  'settings.externalControlHint':
    'Raycast や Alfred などのランチャーから tomari:// URL スキームで前面ウィンドウを操作できるようにします。外部アプリからの操作を受け付ける設定なので、使うときだけオンにしてください。',
  'settings.externalWindowActions': 'URL スキームでの操作',

  'settings.maintenance': 'メンテナンス',
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
  'settings.applyWarning.launchAtLogin':
    'ログイン時に起動の設定は保存しましたが、システムに適用できませんでした。オフにしてからもう一度オンにすると再試行します。',
  'settings.applyWarning.menuBar':
    'メニューバーの設定は保存しましたが、適用できませんでした。オフにしてからもう一度オンにすると再試行します。',
  'settings.applyWarning.keyboardTap':
    'キーボードカスタマイズは保存しましたが、イベントタップを開始できませんでした。入力監視の許可を確認して、オフにしてからもう一度オンにしてください。',
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
