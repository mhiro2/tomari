// Minimal i18n: a typed English dictionary, a Japanese mirror enforced by the
// type checker, and a context that hands components a translate function.

import { createContext, useContext, type ReactNode } from 'react';

import type { Language } from './types';

const en = {
  'app.tabs.keyboard': 'Keyboard',
  'app.tabs.window': 'Windows',
  'app.tabs.settings': 'Settings',
  'app.sections': 'Sections',
  'app.featureOff': 'Off',

  'common.add': 'Add',
  'common.delete': 'Delete',
  'common.cancel': 'Cancel',
  'common.label': 'Label',
  'common.loading': 'Loading…',
  'common.enable': 'Enable {label}',
  'common.turnOn': 'Turn on',

  'preset.leftHalf': 'Left Half',
  'preset.rightHalf': 'Right Half',
  'preset.topHalf': 'Top Half',
  'preset.bottomHalf': 'Bottom Half',
  'preset.topLeftQuarter': 'Top Left',
  'preset.topRightQuarter': 'Top Right',
  'preset.bottomLeftQuarter': 'Bottom Left',
  'preset.bottomRightQuarter': 'Bottom Right',
  'preset.leftThird': 'Left Third',
  'preset.centerThird': 'Center Third',
  'preset.rightThird': 'Right Third',
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
  'action.undoWindow': 'Undo Window Move',
  'action.launch': 'Launch: {app}',
  'action.quickPeek': 'Quick Peek: {app}',
  'action.sendKeystroke': 'Send: {keys}',
  'action.ime': 'IME → {mode}',
  'action.toggleKeepAwake': 'Toggle keep awake',
  'action.noOp': 'Do Nothing',

  'keyboard.modifierKeys': 'Modifier keys',
  'keyboard.modifierHint':
    'Tap a modifier alone to fire its command; hold it and system shortcuts keep working.',
  'keyboard.globalShortcuts': 'Global shortcuts',
  'keyboard.tap': 'Tap',
  'keyboard.actsAsHyper': 'acts as Hyper (⌃⌥⇧⌘)',
  'keyboard.actsAs': 'acts as {modifier}',
  'keyboard.shortcutLabelAria': 'Shortcut label',
  'keyboard.actionAria': 'Action',
  'keyboard.recordShortcut': 'Record shortcut',
  'keyboard.changeShortcut': 'Change shortcut for {label}',
  'keyboard.offNote': "Keyboard customization is off — taps and shortcuts won't fire.",
  'keyboard.holdThreshold': 'Tap / hold threshold',
  'keyboard.holdThresholdDesc':
    'How long a modifier must be held to count as a hold instead of a tap.',

  'recorder.click': 'Click to record',
  'recorder.typing': 'Type shortcut…',
  'recorder.needModifier': 'Add ⌃, ⌥ or ⌘ — or use a function key',
  'recorder.unsupported': 'Unsupported shortcut',
  'recorder.startFailed': 'Could not start recording',

  'error.permissionRequired': 'Accessibility permission is required.',
  'error.noFocusedWindow': "There's no window to act on.",
  'error.shortcutConflict':
    "That shortcut couldn't be registered — it may conflict with another app.",

  'window.axNeeded': 'Accessibility access needed',
  'window.axBody': "Tomari moves other apps' windows through macOS Accessibility.",
  'window.grantAccess': 'Grant access',
  'window.offNote': "Window management is off — snapping and display moves won't work.",
  'window.snapSection': 'Snap focused window',
  'window.cycleHint': 'Snapping Left/Right Half again cycles ½ → ⅓ → ⅔.',
  'window.displaysSection': 'Displays & history',
  'window.prevDisplay': '← Previous display',
  'window.nextDisplay': 'Next display →',
  'window.undoMove': '↩ Undo last move',
  'window.snappedTo': 'Snapped to {label}',
  'window.disabled': 'Window management is disabled',
  'window.movedPrev': 'Moved to previous display',
  'window.movedNext': 'Moved to next display',
  'window.restored': 'Restored previous position',
  'window.dragToSnap': 'Drag to snap',
  'window.dragToSnapToggle': 'Snap by dragging to a screen edge',
  'window.enableDragToSnap': 'Enable drag to snap',
  'window.dragToSnapHint':
    'Drag a window to a screen edge or corner — a preview appears and the window snaps to a half, corner, or full screen when you let go. Requires Accessibility and Input Monitoring.',

  'settings.general': 'General',
  'settings.launchAtLogin': 'Launch at login',
  'settings.showInMenuBar': 'Show in menu bar',
  'settings.hiddenHint':
    'Hidden from the menu bar — open Tomari with the global shortcut (default ⌘⇧K).',
  'settings.appearance': 'Appearance',
  'settings.theme.system': 'System',
  'settings.theme.light': 'Light',
  'settings.theme.dark': 'Dark',
  'settings.language': 'Language',
  'settings.language.system': 'System',
  'settings.keyboardCustomization': 'Keyboard customization',
  'settings.windowManagement': 'Window management',

  'settings.session': 'Session',
  'settings.keepAwakeToggle': 'Prevent idle sleep',
  'settings.keepAwakeHint':
    "Keeps your Mac — and anything it's running — awake even with the lid closed. Asks for your administrator password when turning it on. Expect more battery use and heat.",
  'settings.lidClose': 'Work with lid closed',
  'settings.lidActive': 'Active',
  'settings.lidPending': 'Enabling…',
  'settings.lidUnavailable': 'Unavailable',
  'settings.lidOff': 'Off',
  'settings.keepAwakeNoLidClose':
    "Staying awake with the lid closed couldn't be enabled (administrator approval was declined). Sleep is still prevented while the lid is open.",

  'settings.externalControl': 'External control',
  'settings.externalControlHint':
    "Let launchers like Raycast and Alfred place the focused window through the tomari:// URL scheme. This is a security boundary — leave it off if you don't use it.",
  'settings.externalWindowActions': 'URL scheme control',

  'settings.maintenance': 'Maintenance',
  'settings.version': 'Version',
  'settings.updateAvailable': 'Version {version} is available.',
  'settings.updateFailed': 'Update failed: {error}',
  'settings.installRestart': 'Install and restart',
  'settings.installing': 'Installing…',
  'settings.upToDate': 'Tomari is up to date.',
  'settings.updateCheckFailed': 'Could not check for updates: {error}',
  'settings.checkUpdates': 'Check for updates',
  'settings.checking': 'Checking…',
  'settings.saveFailed': 'Could not save settings: {error}',

  'settings.backupHint':
    'Export every setting to a single JSON file to keep, or import one to restore. Importing replaces your current configuration.',
  'settings.export': 'Export…',
  'settings.import': 'Import…',
  'settings.working': 'Working…',
  'settings.importConfirm':
    'Importing replaces your current keyboard and window configuration. The current one is backed up automatically first.',
  'settings.importContinue': 'Replace and import',
  'settings.exportSaved': 'Saved to {path}',
  'settings.exportOmitted': '{count} unreadable entries were left out of the file.',
  'settings.importApplied': 'Imported {hotkeys} hotkeys and {modifierRules} modifier rules.',
  'settings.importBackedUp': 'Your previous configuration was backed up to {path}.',
  'settings.importRejected': 'The file was not imported — fix these and try again:',
  'settings.importWarnings': 'Notes:',
  'settings.importRegFailures': 'Some shortcuts could not be registered (a conflict, perhaps):',
  'settings.backupFailed': 'Operation failed: {error}',
} as const;

export type MessageKey = keyof typeof en;

const ja: Record<MessageKey, string> = {
  'app.tabs.keyboard': 'キーボード',
  'app.tabs.window': 'ウィンドウ',
  'app.tabs.settings': '設定',
  'app.sections': 'セクション',
  'app.featureOff': 'オフ',

  'common.add': '追加',
  'common.delete': '削除',
  'common.cancel': 'キャンセル',
  'common.label': 'ラベル',
  'common.loading': '読み込み中…',
  'common.enable': '{label} を有効化',
  'common.turnOn': 'オンにする',

  'preset.leftHalf': '左半分',
  'preset.rightHalf': '右半分',
  'preset.topHalf': '上半分',
  'preset.bottomHalf': '下半分',
  'preset.topLeftQuarter': '左上',
  'preset.topRightQuarter': '右上',
  'preset.bottomLeftQuarter': '左下',
  'preset.bottomRightQuarter': '右下',
  'preset.leftThird': '左 1/3',
  'preset.centerThird': '中央 1/3',
  'preset.rightThird': '右 1/3',
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
  'action.moveToDisplay': '{display}へ移動',
  'action.undoWindow': 'ウィンドウ移動を元に戻す',
  'action.launch': '起動: {app}',
  'action.quickPeek': 'Quick Peek: {app}',
  'action.sendKeystroke': '送信: {keys}',
  'action.ime': 'IME → {mode}',
  'action.toggleKeepAwake': 'スリープ防止の切り替え',
  'action.noOp': '何もしない',

  'keyboard.modifierKeys': '修飾キー',
  'keyboard.modifierHint':
    '修飾キーを単独で押すとコマンドを実行します。長押しすると通常の修飾キーとして使えます。',
  'keyboard.globalShortcuts': 'グローバルショートカット',
  'keyboard.tap': '押す',
  'keyboard.actsAsHyper': '長押しで Hyper (⌃⌥⇧⌘)',
  'keyboard.actsAs': '長押しで {modifier}',
  'keyboard.shortcutLabelAria': 'ショートカットのラベル',
  'keyboard.actionAria': 'アクション',
  'keyboard.recordShortcut': 'ショートカットを記録',
  'keyboard.changeShortcut': '{label} のショートカットを変更',
  'keyboard.offNote': 'キーボードカスタマイズはオフです。タップ・ショートカットは実行されません。',
  'keyboard.holdThreshold': '長押し判定の時間',
  'keyboard.holdThresholdDesc':
    '修飾キーをこの時間以上押し続けると、タップではなく長押しと判定します。',

  'recorder.click': 'クリックして記録',
  'recorder.typing': 'ショートカットを入力…',
  'recorder.needModifier': '⌃ ⌥ ⌘ のいずれかを加えるか、ファンクションキーを使ってください',
  'recorder.unsupported': 'このショートカットは使えません',
  'recorder.startFailed': '記録を開始できませんでした',

  'error.permissionRequired': 'アクセシビリティの許可が必要です。',
  'error.noFocusedWindow': '操作対象のウィンドウがありません。',
  'error.shortcutConflict':
    'このショートカットを登録できませんでした。他のアプリと競合している可能性があります。',

  'window.axNeeded': 'アクセシビリティへのアクセスが必要です',
  'window.axBody': 'Tomari は macOS のアクセシビリティ機能で他のアプリのウィンドウを操作します。',
  'window.grantAccess': 'アクセスを許可',
  'window.offNote': 'ウィンドウ管理はオフです。スナップ・ディスプレイ移動は動作しません。',
  'window.snapSection': '前面ウィンドウをスナップ',
  'window.cycleHint': '左右半分のスナップを繰り返すと ½ → ⅓ → ⅔ と順に切り替わります。',
  'window.displaysSection': 'ディスプレイと履歴',
  'window.prevDisplay': '← 前のディスプレイ',
  'window.nextDisplay': '次のディスプレイ →',
  'window.undoMove': '↩ 直前の移動を元に戻す',
  'window.snappedTo': '{label} にスナップしました',
  'window.disabled': 'ウィンドウ管理は無効です',
  'window.movedPrev': '前のディスプレイへ移動しました',
  'window.movedNext': '次のディスプレイへ移動しました',
  'window.restored': '元の位置に戻しました',
  'window.dragToSnap': 'ドラッグスナップ',
  'window.dragToSnapToggle': '画面端へのドラッグでスナップ',
  'window.enableDragToSnap': 'ドラッグスナップを有効化',
  'window.dragToSnapHint':
    'ウィンドウを画面の端や隅にドラッグすると、プレビューが表示され、離した位置に応じて左右半分・四隅・全画面にスナップします。アクセシビリティと入力監視の権限が必要です。',

  'settings.general': '一般',
  'settings.launchAtLogin': 'ログイン時に起動',
  'settings.showInMenuBar': 'メニューバーに表示',
  'settings.hiddenHint':
    'メニューバー非表示中は、グローバルショートカット（デフォルト ⌘⇧K）で Tomari を開けます。',
  'settings.appearance': '外観',
  'settings.theme.system': 'システム',
  'settings.theme.light': 'ライト',
  'settings.theme.dark': 'ダーク',
  'settings.language': '言語',
  'settings.language.system': 'システム',
  'settings.keyboardCustomization': 'キーボードカスタマイズ',
  'settings.windowManagement': 'ウィンドウ管理',

  'settings.session': 'セッション',
  'settings.keepAwakeToggle': 'アイドルスリープを防止',
  'settings.keepAwakeHint':
    'ディスプレイを閉じても、Mac と実行中の処理をスリープさせません。オンにするとき管理者パスワードを尋ねます。バッテリー消費と発熱が増える点に注意してください。',
  'settings.lidClose': '蓋を閉じても継続',
  'settings.lidActive': '有効',
  'settings.lidPending': '有効化中…',
  'settings.lidUnavailable': '利用不可',
  'settings.lidOff': 'オフ',
  'settings.keepAwakeNoLidClose':
    'ディスプレイを閉じた状態でのスリープ防止は有効化できませんでした（管理者の許可が得られませんでした）。ディスプレイを開いている間はスリープを防止します。',

  'settings.externalControl': '外部制御',
  'settings.externalControlHint':
    'Raycast や Alfred などのランチャーから tomari:// URL スキームで前面ウィンドウを操作できるようにします。セキュリティ境界です。使わない場合はオフのままにしてください。',
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

  'settings.backupHint':
    'すべての設定を 1 つの JSON ファイルに書き出して保存したり、読み込んで復元できます。インポートは現在の設定を置き換えます。',
  'settings.export': 'エクスポート…',
  'settings.import': 'インポート…',
  'settings.working': '処理中…',
  'settings.importConfirm':
    'インポートは現在のキーボード・ウィンドウ設定を置き換えます。事前に現在の設定を自動でバックアップします。',
  'settings.importContinue': '置き換えてインポート',
  'settings.exportSaved': '{path} に保存しました',
  'settings.exportOmitted': '読み取れなかった項目 {count} 件はファイルから除外されました。',
  'settings.importApplied':
    '{hotkeys} 件のホットキー、{modifierRules} 件の修飾ルールを読み込みました。',
  'settings.importBackedUp': '以前の設定は {path} にバックアップしました。',
  'settings.importRejected': 'ファイルは読み込まれませんでした。修正して再度お試しください:',
  'settings.importWarnings': 'メモ:',
  'settings.importRegFailures':
    '一部のショートカットを登録できませんでした（競合の可能性があります）:',
  'settings.backupFailed': '処理に失敗しました: {error}',
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
