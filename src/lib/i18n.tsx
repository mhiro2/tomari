// Minimal i18n: a typed English dictionary, a Japanese mirror enforced by the
// type checker, and a context that hands components a translate function.

import { createContext, useContext, type ReactNode } from 'react';

import type { Language } from './types';

const en = {
  'display.profiles': 'Profiles',
  'display.external': 'External display',
  'display.unnamedProfile': 'Unnamed configuration',
  'display.systemSettings': 'Open macOS Display settings…',
  'display.identify': 'Identify',
  'display.ambiguousIdentity':
    'macOS reports the same identity for multiple screens. Controls for those screens and changes to the whole display configuration are unavailable.',
  'display.incompleteConfiguration':
    'macOS could not report the current resolution for every display. Refresh before changing or confirming the display configuration.',
  'display.identifyHint': 'Show these numbers on the connected screens for three seconds.',
  'display.profilesHint':
    'Save resolution, refresh rate, main display, and arrangement together. Profiles apply only to the same connected displays and always use a 20-second preview. Brightness and extra dimming are not saved.',
  'display.profileName': 'Profile name',
  'display.profilePlaceholder': 'For example, Focus or Presentation',
  'display.saveProfile': 'Save current configuration',
  'display.savedProfiles': 'Saved configurations',
  'display.profileContents': 'Saved display settings',
  'display.disconnectedScreen': 'Disconnected display',
  'display.profileReady':
    'Ready for {count} connected displays. Saved modes are checked when applied.',
  'display.applyProfile': 'Preview configuration',
  'display.deleteProfile': 'Delete profile',
  'display.renameProfile': 'Rename profile',
  'display.newProfileName': 'New profile name',
  'display.saveProfileName': 'Save name',
  'display.deleteProfileHint': 'Delete “{name}”? Your current display settings stay as they are.',
  'display.noProfiles': 'No saved configurations yet.',
  'display.profileLimit': 'You can save up to 32 configurations. Delete one before saving another.',
  'display.clearProfiles': 'Reset saved profiles',
  'display.clearProfilesHint':
    'Delete all saved display profiles? Current display settings are unaffected.',
  'display.error.invalidProfile':
    'Use a name of 1–80 characters and a valid extended display configuration.',
  'display.error.profileUnavailable':
    'Connect exactly the displays used by this saved configuration.',
  'display.error.profileStorage':
    'Saved display profiles could not be read or written. Retry, or reset saved profiles if their data is damaged.',
  'display.error.refreshRequired':
    'The change was sent, but the current display state could not be read. Refresh to check it.',
  'display.controls': 'Display controls',
  'display.singleDisplay': 'Connect another display to arrange your desktop.',
  'display.dimming': 'Extra dimming',
  'display.dimmingHint':
    'Dim the image with a software shade, even when hardware brightness is unavailable. This does not reduce the backlight’s power use.',
  'display.dimmingSession':
    'Session only. Resets when Tomari quits; capped at 80% so the screen stays visible.',
  'display.applyDimming': 'Apply dimming',
  'display.resetDimming': 'Reset all dimming',
  'display.error.invalidDimming': 'Choose software dimming between 0 and 80%.',

  'display.refreshRate': 'Refresh rate',
  'display.automaticRate': 'Automatic / variable',
  'display.showLowResolution': 'Show non-HiDPI resolutions',

  'display.position': 'Arrangement',
  'display.positionHint': 'Place this display beside another. macOS keeps the desktop connected.',
  'display.relativeTo': 'Next to',
  'display.placement': 'Position',
  'display.left': 'Left',
  'display.right': 'Right',
  'display.above': 'Above',
  'display.below': 'Below',
  'display.tryPosition': 'Try arrangement',
  'display.makeMain': 'Use as main display',
  'display.mainHint': 'The main display anchors the desktop arrangement.',
  'display.error.invalidArrangement':
    'Choose a different connected display as the arrangement reference.',

  'app.nav.display': 'Displays',
  'display.title': 'Displays',
  'display.description':
    'Choose a comfortable resolution and brightness for each connected display.',
  'display.arrangement': 'Your displays',
  'display.builtIn': 'Built-in display',
  'display.main': 'Main display',
  'display.mirrored': 'Mirroring',
  'display.select': 'Select {name}',
  'display.resolution': 'Resolution',
  'display.resolutionHint':
    'HiDPI keeps text sharp. Available modes come from macOS and your display.',
  'display.current': 'Current: {mode}',
  'display.pixels': 'Rendered at {width} × {height} pixels',
  'display.preview': 'Try resolution',
  'display.previewTitle': 'Keep this display setting?',
  'display.previewHint':
    'Reverting in {seconds} seconds. The timer continues if you leave this page.',
  'display.keep': 'Keep changes',
  'display.revert': 'Revert now',
  'display.brightness': 'Brightness',
  'display.brightnessHint': 'Adjust the display’s hardware brightness.',
  'display.brightnessUnavailable':
    'Hardware brightness is unavailable through macOS for this display. Use the display’s own controls.',
  'display.applyBrightness': 'Apply brightness',
  'display.empty': 'No displays are available. Reconnect a display and refresh.',
  'display.loading': 'Reading displays…',
  'display.loadFailed': 'Could not read your displays. Try refreshing.',
  'display.notice.reverted': 'The previous display setting was restored.',
  'display.notice.restoreFailed':
    'The previous setting could not be restored. Try reverting again or use macOS Display settings.',
  'display.notice.displayChanged':
    'The display disconnected or its settings changed elsewhere. The preview was cancelled.',
  'display.error.unavailable': 'Display management is unavailable on this system.',
  'display.error.disconnected': 'This display disconnected. Refresh to see connected displays.',
  'display.error.modeUnavailable':
    'This resolution is no longer available. Refresh and choose another.',
  'display.error.mirrored':
    'Change mirrored displays together in macOS Display settings. Resolution, arrangement, and saved-configuration changes are unavailable while mirroring is active.',
  'display.error.busy': 'Finish the current display preview before making another change.',
  'display.error.previewExpired': 'This preview has ended. Refresh to see the current setting.',
  'display.error.invalidBrightness': 'Choose a brightness between 0 and 100%.',
  'display.error.brightnessUnavailable': 'Hardware brightness is unavailable for this display.',
  'display.error.system': 'macOS could not apply the display change. Refresh and try again.',

  // Sidebar section names. Prevent Sleep is named for what it does, matching
  // the tray entry and the switch inside the section.
  'app.nav.keyboard': 'Keyboard',
  'app.nav.window': 'Windows',
  'app.nav.menubar': 'Menu Bar',
  'app.nav.session': 'Prevent Sleep',
  'app.nav.general': 'General',
  'app.nav.health': 'Diagnostics',
  'app.sections': 'Sections',
  'app.tools': 'Tools',
  'app.app': 'App',
  'app.permissionsReady': 'Permissions: Ready',
  'app.permissionsAttention': 'Permissions: Needs attention',
  'app.permissionsUnknown': 'Permissions: Checking…',

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
  'common.loadFailedTitle': 'Settings unavailable',
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
  'action.openDisplaySettings': 'Open Display Settings',
  'action.resetDisplayDimming': 'Reset All Display Dimming',
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
    'Choose what a modifier key does when tapped or held, and configure global shortcuts.',
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
  'keyboard.addRule': 'Add Modifier Rule',
  'keyboard.editRule': 'Edit Modifier Rule',
  'keyboard.edit': 'Edit',
  'keyboard.editRuleFor': 'Edit modifier rule {label}',
  'keyboard.deleteRule': 'Delete modifier rule {label}',
  'keyboard.ruleSettings': 'Modifier rule settings',
  'keyboard.ruleLabel': 'Modifier rule label',
  'keyboard.ruleEditorHint': 'Choose a physical key, its hold role, and an action for a solo tap.',
  'keyboard.physicalKey': 'Physical key',
  'keyboard.keySide': 'Key side',
  'keyboard.noKeySide': 'No side',
  'keyboard.holdRole': 'While held',
  'keyboard.keepOriginal': 'Keep the original modifier',
  'keyboard.chooseHoldRole': 'Choose one hold role to repair this rule',
  'keyboard.reservedCommandHint':
    'Left and right Command are reserved for the input switch, even when it is off. With Either selected, the input switch takes priority while enabled.',
  'keyboard.sendKeystroke': 'Send a keystroke',
  'keyboard.keystroke': 'Keystroke',
  'keyboard.recordKeystroke': 'Record keystroke to send',
  'keyboard.keystrokeHint':
    'Record a key or enter a combination such as Escape, Enter, or Cmd+Space. Escape is recorded here; click the recorder again or move focus to cancel.',
  'keyboard.ruleEnabled': 'Enable this rule',
  'keyboard.saveRule': 'Save Rule',
  'keyboard.savingRule': 'Saving…',
  'keyboard.repairRule': 'Repair',
  'keyboard.repairRuleFor': 'Repair modifier rule {label}',
  'keyboard.createReplacement': 'Create New Rule',
  'keyboard.createReplacementFor': 'Create a new rule for {label}',
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
  'keyboard.configurationWarningTitle': 'Saved keyboard items need attention ({count})',
  'keyboard.configurationWarningBody':
    'Invalid saved items were not deleted. Keyboard items without problems continue to work.',
  'keyboard.configurationWarningAction': 'Review in Keyboard',
  'keyboard.configurationIssuesTitle': 'Some saved keyboard items are not running',
  'keyboard.configurationIssuesBody':
    'Tomari skipped the items below to keep keyboard input safe. They remain saved, and items without problems continue to work.',
  'keyboard.configurationIssuesHotkeys': 'Shortcuts',
  'keyboard.configurationIssuesModifierRules': 'Modifier keys',
  'keyboard.deleteInvalidItem': 'Delete invalid item {label}',
  'keyboard.configurationIssueUnnamed': 'Unnamed item ({id})',
  'keyboard.configurationIssueUnknown': 'Saved item with no ID',
  'keyboard.configurationIssueReason.emptyId': 'This item has no ID.',
  'keyboard.configurationIssueReason.idTooLong': 'The item ID is too long.',
  'keyboard.configurationIssueReason.emptyLabel': 'The item has no label.',
  'keyboard.configurationIssueReason.labelTooLong': 'The label is too long.',
  'keyboard.configurationIssueReason.invalidAccelerator': 'The shortcut format is invalid.',
  'keyboard.configurationIssueReason.unsafeGlobalShortcut':
    'The shortcut could intercept ordinary typing.',
  'keyboard.configurationIssueReason.invalidKeystroke': 'The keystroke action is invalid.',
  'keyboard.configurationIssueReason.reservedRuleId': 'The rule ID is reserved by Tomari.',
  'keyboard.configurationIssueReason.hyperWithRemap':
    'Hyper and a different hold key cannot be used together.',
  'keyboard.configurationIssueReason.reservedCommandSlot':
    'This Command-key slot is reserved by the built-in input switch.',
  'keyboard.configurationIssueReason.duplicateId':
    'More than one saved item has this ID after normalization.',
  'keyboard.configurationIssueReason.duplicateAccelerator':
    'More than one saved item uses this shortcut.',
  'keyboard.configurationIssueReason.duplicateModifierSlot':
    'More than one saved rule uses this modifier-key slot.',
  'keyboard.configurationIssueReason.unsupportedModifierSide':
    'Caps Lock and fn have no left or right side. Choose No side.',

  'recorder.click': 'Click to record',
  'recorder.typing': 'Type shortcut…',
  'recorder.typingKeystroke': 'Press the key to send…',
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
  'error.settingsRecoveryRequired': 'Settings must be repaired before changes can be saved.',
  'error.databaseResetRequired':
    'The quarantined settings database must be reset before Tomari can continue.',
  'error.backupInvalid':
    'This is not a supported, valid Tomari settings backup. Choose another file.',
  'error.backupCurrentInvalid':
    'Current settings need repair before they can be backed up or replaced. Review Diagnostics and Keyboard.',
  'error.backupPreviewExpired':
    'Settings changed after this preview. Review the replacement again before applying it.',
  'error.backupUnavailable': 'There are no previous settings available to restore.',

  'backup.title': 'Settings backup',
  'backup.privacy':
    'Settings backups contain your shortcut bindings, labels, app identifiers, and display identities. They are different from the anonymous support bundle in Diagnostics.',
  'backup.exportTitle': 'Back up your configuration',
  'backup.exportDescription':
    'Save general settings, shortcuts, modifier rules, remembered window positions, and display profiles as a JSON file.',
  'backup.export': 'Export Settings',
  'backup.exporting': 'Exporting…',
  'backup.exported': 'Settings backup saved.',
  'backup.openFolder': 'Open Backup Folder',
  'backup.importTitle': 'Restore from a backup file',
  'backup.importDescription':
    'Choose a Tomari settings backup, review the changes, then replace the current configuration.',
  'backup.chooseFile': 'Choose Backup…',
  'backup.checking': 'Checking Backup…',
  'backup.restoreTitle': 'Return to the settings before the last import',
  'backup.restoreDescription':
    'Review and restore the configuration saved immediately before your last import. Later edits will also be replaced.',
  'backup.restore': 'Restore Previous Settings…',
  'backup.noRestore': 'No previous import is available to undo.',
  'backup.statusFailed': 'Could not check for previous settings: {error}',
  'backup.imported': 'Settings replaced. Use “Restore Previous Settings” to undo this import.',
  'backup.restored': 'The settings from before the last import have been restored.',
  'backup.applyUnconfirmed':
    'The result could not be confirmed. Current settings have been refreshed. Review them, or use “Restore Previous Settings” if available.',
  'backup.fileTooLarge': 'Choose a settings backup no larger than 2 MiB.',
  'backup.busy': 'Completing the settings operation…',
  'backup.confirmImportTitle': 'Replace all settings from this backup?',
  'backup.confirmImportBody':
    'This replaces your entire saved configuration, including items with the same count. Your current configuration will be saved so you can restore it later.',
  'backup.confirmRestoreTitle': 'Restore the settings before the last import?',
  'backup.confirmRestoreBody':
    'This replaces your entire configuration, including changes made since the import. This restore can be used once. Export your current settings first if you want to keep them.',
  'backup.items': 'Saved items',
  'backup.current': 'Current',
  'backup.replacement': 'After replacement',
  'backup.hotkeys': 'Shortcuts',
  'backup.modifierRules': 'Modifier rules',
  'backup.windowPlacements': 'Window positions',
  'backup.displayProfiles': 'Display profiles',
  'backup.settingsChanges': 'General settings that will change',
  'backup.settingsUnchanged':
    'General settings have the same values. The saved items above will still be replaced.',
  'backup.setting': 'Setting',
  'backup.dragToSnap': 'Drag to snap',
  'backup.dragToMove': 'Drag to move and resize',
  'backup.runtimeExcluded':
    'Permissions, Caps Lock ownership, and the current Prevent Sleep session are not imported or restored. Saved display profiles are replaced without changing the active displays.',
  'backup.hiddenIcon':
    'This configuration hides the menu bar icon. Launch Tomari again from Spotlight to reopen Settings.',
  'backup.reviewAgain': 'Review Again',
  'backup.replaceAll': 'Replace All Settings',
  'backup.applying': 'Replacing Settings…',

  'recovery.eyebrow': 'Safety interlock',
  'recovery.title': 'Settings need repair',
  'recovery.intro':
    "Tomari couldn't safely read its saved settings. Nothing unreadable has been replaced.",
  'recovery.databaseResetIntro':
    'Tomari found a damaged settings database and preserved it for manual recovery.',
  'recovery.pausedTitle': 'Automation is paused',
  'recovery.pausedBody':
    'Keyboard gestures, shortcuts, window controls, and menu bar automation are not running.',
  'recovery.optionsTitle': 'Choose how to continue',
  'recovery.optionsBody':
    'Try again without changing anything. Reset may replace affected preferences, shortcuts, or modifier rules with safe defaults. Readable window positions are kept unless the database itself is unusable.',
  'recovery.databaseResetOptionsTitle': 'Reset to continue',
  'recovery.databaseResetOptionsBody':
    'The damaged database cannot be retried safely. Confirm a reset to initialize its replacement with automation-off defaults.',
  'recovery.retry': 'Try Again',
  'recovery.retrying': 'Trying Again…',
  'recovery.reset': 'Reset Settings…',
  'recovery.confirmTitle': 'Reset unreadable settings?',
  'recovery.confirmBody':
    'Tomari will reset affected data to safe defaults. Shortcuts or rules may be replaced, and readable window positions are kept unless the database must be quarantined. Automation stays off until you turn each feature back on.',
  'recovery.confirmAction': 'Reset and Continue',
  'recovery.resetting': 'Resetting…',
  'recovery.retryFailed': 'Tomari still could not read the settings: {error}',
  'recovery.resetFailed': 'Tomari could not reset the settings: {error}',

  'setup.title': 'Get Tomari ready',
  'setup.intro':
    'Allow two macOS permissions so Tomari can move windows and recognize key gestures. Display management works without these permissions; choose Set up later to use it now.',
  'setup.accessibility': 'Accessibility',
  'setup.accessibilityWhy':
    'Lets Tomari move and resize windows, send keys, and read or rearrange your menu bar items.',
  'setup.inputMonitoring': 'Input Monitoring',
  'setup.inputMonitoringWhy':
    'Lets Tomari recognize modifier-key taps and holds, the hyper key, and window drag gestures.',
  'setup.granted': 'Granted',
  'setup.grant': 'Open System Settings',
  'setup.grantFor': 'Open System Settings for {name}',
  'setup.requesting': 'Opening…',
  'setup.openAgain': 'Open Again',
  'setup.returnHint': 'After allowing it in System Settings, return to Tomari.',
  'setup.requestedHint':
    'If Tomari still cannot use a permission, restart it. If it still appears ungranted after updating Tomari, remove the old Tomari entry in System Settings and add the current app again, even if its switch is on.',
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
    'Window controls, key sending, and menu bar inspection or rearrangement stay limited until it is allowed.',
  'setup.bannerDescription.inputMonitoring':
    'Modifier-key gestures and window-drag tools stay limited until it is allowed.',
  'setup.bannerAction': 'Review permissions',
  'setup.openSetup': 'Open Setup',
  'setup.updateRegrant':
    "Tomari's permissions went missing after the update — a known limitation for now. Please grant them again.",
  'setup.adminNote':
    'Prevent Sleep needs no permission for ordinary use with the lid open. Only the lid-closed mode asks for your administrator password when changing the lid-close setting.',

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
  'window.homeDamaged':
    'The saved position is damaged and cannot be used. Replace it or forget it.',
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
  'menubar.unsupportedOs':
    'macOS 27 no longer lets an app push status icons off the menu bar, so menu bar ' +
    'tidying is unavailable. When your icons no longer fit, use the expand button ' +
    'macOS shows beside them.',
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
    'Keep downloads, builds, and other long jobs running. Choose whether to keep working with the MacBook lid closed. Sessions are temporary and do not restart when Tomari opens.',
  'settings.keepAwakeMode': 'Prevent sleep while',
  'settings.keepAwakeIdle': 'Lid is open',
  'settings.keepAwakeLid': 'Lid is open or closed',
  'settings.keepAwakeIdleHint':
    'Prevents idle sleep without administrator approval. Closing the lid and choosing Sleep still work normally. Stop the session before changing modes.',
  'settings.keepAwakeLidHint':
    'Also prevents sleep when the MacBook lid closes. Changing the lid-close setting requires administrator approval when starting or stopping. Stop the session before changing modes.',
  'settings.keepAwakeModeRecoveryHint':
    'Resolve the previous lid-close setting before changing modes. Restoring it requires administrator approval.',
  'settings.keepAwakeAction': 'Keep this Mac awake',
  'settings.keepAwakeStart': 'Start Preventing Sleep',
  'settings.keepAwakeStartAdmin': 'Start Preventing Sleep…',
  'settings.keepAwakeStop': 'Stop Preventing Sleep',
  'settings.keepAwakeStopAdmin': 'Stop Preventing Sleep…',
  'settings.keepAwakeActive': 'Sleep is being prevented',
  'settings.keepAwakeInactive': 'Your Mac sleeps normally',
  'settings.keepAwakeEnabling': 'Turning on…',
  'settings.keepAwakeDisabling': 'Turning off…',
  'settings.keepAwakeEnablingAdmin': 'Turning on — waiting for administrator approval.',
  'settings.keepAwakeDisablingAdmin': 'Turning off — waiting for administrator approval.',
  'settings.keepAwakeFailed': 'The last change was not applied',
  'settings.lidClose': 'Work with lid closed',
  'settings.lidActive': 'Active',
  'settings.lidPending': 'Enabling…',
  'settings.lidUnavailable': 'Unavailable',
  'settings.lidOff': 'Off',
  'settings.authorizationPendingHint':
    'Complete the macOS prompt, or cancel to return to the previous state.',
  'settings.safetyGuards': 'Allow sleep again automatically',
  'settings.autoOff': 'After a set time',
  'settings.autoOffHint':
    'Allows sleep again once the time is up. Tomari keeps track even while this window is closed.',
  'settings.autoOffNever': 'Never',
  'settings.autoOff30m': 'After 30 minutes',
  'settings.autoOff1h': 'After 1 hour',
  'settings.autoOffAtTime': 'At a specific time…',
  'settings.endTime': 'Allow sleep at',
  'settings.timeRemaining': 'Time remaining',
  'settings.acOnly': 'When unplugged from power',
  'settings.acOnlyHint':
    'Allows sleep again as soon as the Mac runs on battery. Turning on also requires a power adapter.',
  'settings.lowBattery': 'When battery reaches 20%',
  'settings.lowBatteryHint': 'What happens when the battery drops to 20% or less.',
  'settings.lowBatteryWarn': 'Warn only',
  'settings.lowBatteryTurnOff': 'Allow sleep again',
  'settings.systemState': 'System state',
  'settings.powerSource': 'Power source',
  'settings.powerAc': 'Power adapter',
  'settings.powerBattery': 'Battery',
  'settings.systemUnknown': 'Unknown',
  'settings.advancedState': 'Details',
  'settings.kernelState': 'macOS sleep-disabled flag',
  'settings.kernelStateHint':
    'The SleepDisabled value read directly from macOS. Ordinary idle sleep prevention does not change this lid-close setting.',
  'settings.kernelBlocked': 'Lid-close sleep disabled',
  'settings.kernelAllowed': 'Lid-close sleep allowed',
  'settings.ownership': 'Lid-close setting changed by',
  'settings.ownershipHint': 'Tomari only restores the lid-close setting it changed itself.',
  'settings.ownedByTomari': 'Tomari',
  'settings.notOwned': 'Not changed by Tomari',
  'settings.detectedJobs': 'Long-running jobs',
  'settings.detectedJobsHint': 'Developer processes running for at least five minutes.',
  'settings.noDetectedJobs': 'No long-running developer jobs detected.',
  'settings.processDetail': 'PID {pid} · running {elapsed}',
  'settings.noticeAcRequired': 'Connect a power adapter before turning this on.',
  'settings.noticeAcDisconnected': 'The power adapter was unplugged, so sleep is allowed again.',
  'settings.noticeLowBattery': 'Battery is at or below 20%.',
  'settings.noticeTimerElapsed': 'The set time is up, so sleep is allowed again.',
  'settings.noticeAuthorizationDeclined':
    'The administrator change was declined or could not be confirmed.',
  'settings.noticeLidCloseUnconfirmed':
    'The lid-close sleep setting could not be confirmed and may still be active. Retry to restore it.',
  'settings.noticeAssertionFailed':
    'macOS could not apply the sleep-prevention change. Check the current state and try again.',
  'settings.noticeLeftoverOverride':
    'Sleep with the lid closed is currently disabled on this Mac. A previous Tomari session may have left it that way, but it could also have been set by you or another app since. Choose whether to turn sleep back on or leave it as it is.',
  'settings.leftoverClear': 'Turn sleep back on',
  'settings.leftoverKeep': 'Leave it as it is',

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

  'app.restart': 'Restart Tomari',
  'app.restarting': 'Restarting Tomari…',
  'app.restartFailed': 'Tomari could not restart: {error}',
  'app.restartHint':
    'Restart Tomari to try starting these features again. If the problem continues, open Diagnostics.',
  'settings.openPermissionSettings': 'Open Permission Settings',
  'settings.openDiagnostics': 'Open Diagnostics',
  'settings.applyWarning.launchAtLogin':
    'Tomari could not change automatic startup. Check Login Items in System Settings.',
  'settings.applyWarning.menuBar':
    'Menu Bar organization is not working. Diagnostics can check whether this macOS version supports it.',
  'settings.applyWarning.keyboardTap':
    'Keyboard customization is not running. Modifier-key taps, holds, and remaps may not work.',
  'settings.applyWarning.globalShortcuts':
    'Some shortcuts are unavailable. Another app may be using the same keys; review Shortcuts in Keyboard settings.',
  'settings.applyWarning.dragToSnapTap':
    'Drag to snap is not running. Dragging a window to the screen edge will not snap it.',
  'settings.applyWarning.dragToMoveTap':
    'Drag to move is not running. Dragging with modifier keys will not move or resize windows.',
  'settings.applyWarning.capsLockRemap':
    'The Caps Lock assignment is unavailable. Caps Lock may still use its previous behavior.',
  'settings.applyWarning.commandImeRules':
    'Command-key input switching is not working. Tapping Command may not change the input source.',
  'settings.applyWarning.generic':
    'Tomari could not start one of its features. Open Diagnostics to check what is available.',

  'health.title': 'Diagnostics',
  'health.pageDescription':
    'Check that each Tomari feature is working and that macOS has granted what it needs.',
  'health.reading': 'Reading live system health…',
  'health.loadFailed': 'Could not read diagnostics: {error}',
  'health.allReady': 'No health issues detected',
  'health.needsAttention': '{count} items need action',
  'health.needsAttentionOne': '1 item needs action',
  'health.pendingSummary': '{count} health checks are in progress',
  'health.pendingSummaryOne': '1 health check is in progress',
  'health.snapshotMeta': 'Tomari {version} · {architecture} · checked just now',
  'health.refreshFailed': 'Refresh failed; showing the previous snapshot: {error}',
  'health.refreshing': 'Refreshing…',
  'health.tone.ready': 'Ready',
  'health.tone.attention': 'Action needed',
  'health.tone.pending': 'In progress',
  'health.tone.inactive': 'Off',
  'health.group.input': 'Keyboard and mouse',
  'health.group.system': 'System',
  'health.group.support': 'Support',
  'health.permissions': 'macOS permissions',
  'health.permissionsGranted': 'Accessibility and Input Monitoring are granted.',
  'health.permissionsMissingBoth':
    'Accessibility and Input Monitoring are not granted. Open Setup to grant them.',
  'health.permissionsMissingAccessibility':
    'Accessibility is not granted. Open Setup to grant it in System Settings.',
  'health.permissionsMissingInputMonitoring':
    'Input Monitoring is not granted. Open Setup to grant it in System Settings.',
  'health.tap.keyboard': 'Keyboard monitoring',
  'health.tap.dragToSnap': 'Drag-to-snap monitoring',
  'health.tap.dragToMove': 'Drag-to-move monitoring',
  'health.tapOff': 'Turned off in its settings.',
  'health.tapRunning': 'Running.',
  'health.tapCounters':
    'Restarted {restarts} times · paused by macOS {disables} times · recovered {recoveries} times',
  'health.tapState.stopped': 'The feature is on, but Tomari is not receiving input for it.',
  'health.tapState.starting': 'Starting up. Refresh in a moment.',
  'health.tapState.disabledByTimeout': 'macOS paused input monitoring. Tomari is restarting it.',
  'health.tapState.permissionDenied':
    'Input Monitoring or Accessibility is not granted. Open Setup to grant it.',
  'health.tapState.failed':
    'Input monitoring could not start. Turn the feature off and on again in its settings.',
  'health.capsLock': 'Caps Lock remap',
  'health.capsHeld': 'Caps Lock is remapped and working.',
  'health.capsOff': 'Caps Lock is not remapped.',
  'health.capsPending': 'Applying the Caps Lock setting. Wait, then refresh.',
  'health.capsUnknown':
    'Tomari cannot confirm which app controls Caps Lock right now. Open Modifier Keys.',
  'health.capsMismatch': 'Caps Lock is not behaving as configured. Open Modifier Keys.',
  'health.shortcuts': 'Global shortcuts',
  'health.shortcutsOff': 'Keyboard customization is off.',
  'health.shortcutsRegistered': '{registered} shortcuts registered.',
  'health.shortcutsRegisteredOne': '1 shortcut registered.',
  'health.shortcutsInvalid':
    '{invalid} saved shortcuts could not be loaded. Open Shortcuts to review them.',
  'health.shortcutsConflict':
    'Some shortcuts could not be registered. Another app may be using the same keys; {registered} are active.',
  'health.menuBar': 'Menu Bar organization',
  'health.menuBarOff': 'Menu Bar organization is off.',
  'health.menuBarReady': 'Accessibility is granted and the Menu Bar divider is in place.',
  'health.menuBarUnsupported':
    'Menu Bar organization is not supported in this environment, so it stays off.',
  'health.menuBarPermissionMissing':
    'Accessibility permission is required to organize Menu Bar items. Open Setup to grant it.',
  'health.menuBarDividerMissing':
    'Tomari cannot find its Menu Bar divider. Open Menu Bar Settings to restore it.',
  'health.keepAwake': 'Prevent Sleep',
  'health.keepAwake.off': 'Prevent Sleep is off.',
  'health.keepAwake.on': 'Prevent Sleep is on and your Mac will stay awake.',
  'health.keepAwake.idleOn':
    'Idle sleep is prevented. Closing the lid and choosing Sleep still work normally.',
  'health.keepAwake.enabling': 'Prevent Sleep is being enabled. Wait, then refresh.',
  'health.keepAwake.disabling': 'Prevent Sleep is being disabled. Wait, then refresh.',
  'health.keepAwake.failed': 'Prevent Sleep could not be applied. Open its settings and try again.',
  'health.keepAwake.residual':
    'Prevent Sleep is off, but your Mac may still be kept awake. Open its settings.',
  'health.keepAwake.mismatch':
    'Prevent Sleep is not behaving as configured. Open its settings and apply it again.',
  'health.database': 'Saved data',
  'health.databaseReady': 'Your saved data is up to date and intact.',
  'health.databaseFailed': 'Your saved data may be damaged. Export a support bundle.',
  'health.databaseOutdated':
    'Your saved data is in an older format than this version expects. Export a support bundle.',
  'health.databaseUnavailable':
    'Your saved data could not be checked. Export a support bundle for troubleshooting.',
  'health.updates': 'Update verification',
  'health.updateSignatureReady': 'Updates are verified as genuine before they are installed.',
  'health.updateSignatureMissing':
    'This build cannot verify that updates are genuine. Export a support bundle.',
  'health.action.permissions': 'Open Setup',
  'health.action.modifiers': 'Open Modifier Keys',
  'health.action.windowMouse': 'Open Mouse Settings',
  'health.action.shortcuts': 'Open Shortcuts',
  'health.action.menuBar': 'Open Menu Bar Settings',
  'health.action.keepAwake': 'Open Prevent Sleep',
  'health.supportBundle': 'Support bundle',
  'health.supportBundleDescription':
    'Saves this report to a file you can share with support. It does not include what you typed, your shortcuts, menu bar item names, your saved data, error details, or file paths.',
  'health.export': 'Export Support Bundle',
  'health.exporting': 'Exporting…',
  'health.exported': 'Support bundle saved',
  'health.exportFailed': 'Could not export the support bundle: {error}',
} as const;

export type MessageKey = keyof typeof en;

const ja: Record<MessageKey, string> = {
  'display.profiles': '保存した構成',
  'display.external': '外部ディスプレイ',
  'display.unnamedProfile': '名前のない構成',
  'display.systemSettings': 'macOS のディスプレイ設定を開く…',
  'display.identify': '画面を識別',
  'display.ambiguousIdentity':
    '複数の画面に同じ識別子が報告されています。対象画面の操作と、画面構成全体の変更は利用できません。',
  'display.incompleteConfiguration':
    '一部の画面の現在の解像度を macOS から取得できませんでした。画面構成を変更・確定する前に「更新」を押してください。',
  'display.identifyHint': '接続中の画面に、一覧と同じ番号を 3 秒間表示します。',
  'display.profilesHint':
    '解像度・リフレッシュレート・メイン画面・配置をまとめて保存します。同じディスプレイの接続時に呼び出せ、20 秒の確認後に確定します。明るさと追加減光は保存しません。',
  'display.profileName': '構成の名前',
  'display.profilePlaceholder': '例：集中作業、プレゼンテーション',
  'display.saveProfile': '現在の構成を保存',
  'display.savedProfiles': '保存した構成',
  'display.profileContents': '保存された画面設定',
  'display.disconnectedScreen': '未接続のディスプレイ',
  'display.profileReady':
    '接続中の {count} 台に対応しています。適用時に解像度の対応状況を確認します。',
  'display.applyProfile': '構成を試す',
  'display.deleteProfile': '保存した構成を削除',
  'display.renameProfile': '名前を変更',
  'display.newProfileName': '新しい構成の名前',
  'display.saveProfileName': '名前を保存',
  'display.deleteProfileHint': '「{name}」を削除しますか？ 現在の画面設定は変わりません。',
  'display.noProfiles': '保存した構成はまだありません。',
  'display.profileLimit':
    '保存できる構成は 32 件までです。新しく保存するには既存の構成を削除してください。',
  'display.clearProfiles': '保存した構成をリセット',
  'display.clearProfilesHint':
    '保存したディスプレイ構成をすべて削除しますか？ 現在の画面設定は変わりません。',
  'display.error.invalidProfile':
    '名前を 1〜80 文字で入力し、ミラーリングを解除した画面構成で保存してください。',
  'display.error.profileUnavailable': '保存時と同じディスプレイをすべて接続してください。',
  'display.error.profileStorage':
    '保存した構成を読み書きできません。再試行するか、データが破損している場合は保存した構成をリセットしてください。',
  'display.error.refreshRequired':
    '変更を送信しましたが、現在の画面状態を読み取れませんでした。更新して確認してください。',
  'display.controls': 'ディスプレイの設定',
  'display.singleDisplay': 'ほかのディスプレイを接続すると、配置を変更できます。',
  'display.dimming': '追加の減光',
  'display.dimmingHint':
    '本体の明るさを調整できない画面も、ソフトウェアで暗くできます。バックライトの消費電力は変わりません。',
  'display.dimmingSession':
    'Tomari 終了時に解除されます。画面が見えなくならないよう、最大 80% まで減光できます。',
  'display.applyDimming': '減光を適用',
  'display.resetDimming': 'すべての減光を解除',
  'display.error.invalidDimming': '減光は 0〜80% の範囲で選んでください。',

  'display.refreshRate': 'リフレッシュレート',
  'display.automaticRate': '自動・可変',
  'display.showLowResolution': 'HiDPI 以外の解像度も表示',

  'display.position': '配置',
  'display.positionHint':
    'ほかの画面との位置関係を選べます。画面同士がつながるように macOS が調整します。',
  'display.relativeTo': '基準のディスプレイ',
  'display.placement': '位置',
  'display.left': '左',
  'display.right': '右',
  'display.above': '上',
  'display.below': '下',
  'display.tryPosition': '配置を試す',
  'display.makeMain': 'メインディスプレイにする',
  'display.mainHint': 'メインディスプレイを基準に画面が配置されます。',
  'display.error.invalidArrangement': '配置の基準には、接続中の別のディスプレイを選んでください。',

  'app.nav.display': 'ディスプレイ',
  'display.title': 'ディスプレイ',
  'display.description': '接続中の画面ごとに、使いやすい解像度と明るさを選べます。',
  'display.arrangement': '接続中のディスプレイ',
  'display.builtIn': '内蔵ディスプレイ',
  'display.main': 'メインディスプレイ',
  'display.mirrored': 'ミラーリング中',
  'display.select': '{name}を選択',
  'display.resolution': '解像度',
  'display.resolutionHint':
    'HiDPI では文字が鮮明に表示されます。macOS とディスプレイが提供するモードを選べます。',
  'display.current': '現在: {mode}',
  'display.pixels': '描画解像度: {width} × {height} ピクセル',
  'display.preview': '解像度を試す',
  'display.previewTitle': 'この設定を維持しますか？',
  'display.previewHint': 'あと {seconds} 秒で元に戻ります。この画面を閉じても自動で戻ります。',
  'display.keep': '変更を維持',
  'display.revert': '元に戻す',
  'display.brightness': '明るさ',
  'display.brightnessHint': 'ディスプレイ本体の明るさを調整します。',
  'display.brightnessUnavailable':
    'この画面は macOS 経由の明るさ調整に対応していません。ディスプレイ本体の操作で調整してください。',
  'display.applyBrightness': '明るさを適用',
  'display.empty': '利用可能なディスプレイがありません。接続を確認して再読み込みしてください。',
  'display.loading': 'ディスプレイを読み込み中…',
  'display.loadFailed': 'ディスプレイを読み込めませんでした。再読み込みしてください。',
  'display.notice.reverted': '変更前の画面設定に戻しました。',
  'display.notice.restoreFailed':
    '変更前の設定に戻せませんでした。再度「元に戻す」を試すか、macOS のディスプレイ設定を開いてください。',
  'display.notice.displayChanged':
    '画面の接続または設定が外部で変更されたため、プレビューを終了しました。',
  'display.error.unavailable': 'このシステムではディスプレイ管理を利用できません。',
  'display.error.disconnected': 'このディスプレイは切断されました。再読み込みしてください。',
  'display.error.modeUnavailable':
    'この解像度は利用できなくなりました。再読み込みして別の解像度を選んでください。',
  'display.error.mirrored':
    'ミラーリング中は macOS のディスプレイ設定を利用してください。解像度・配置・保存した構成の適用は、ミラーリングを解除すると使えます。',
  'display.error.busy': '現在のプレビューを終了してから変更してください。',
  'display.error.previewExpired':
    'プレビューは終了しました。再読み込みして現在の設定を確認してください。',
  'display.error.invalidBrightness': '明るさは 0〜100% の範囲で選んでください。',
  'display.error.brightnessUnavailable': 'このディスプレイの明るさは調整できません。',
  'display.error.system':
    'macOS が画面設定を変更できませんでした。再読み込みしてもう一度試してください。',

  'app.nav.keyboard': 'キーボード',
  'app.nav.window': 'ウィンドウ',
  'app.nav.menubar': 'メニューバー',
  'app.nav.session': 'スリープ防止',
  'app.nav.general': '一般',
  'app.nav.health': '診断',
  'app.sections': 'セクション',
  'app.tools': 'ツール',
  'app.app': 'アプリ',
  'app.permissionsReady': '権限: 準備完了',
  'app.permissionsAttention': '権限: 要確認',
  'app.permissionsUnknown': '権限: 確認中',

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
  'common.enable': '{label}を有効化',
  'common.loadFailedTitle': '設定を利用できません',
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
  'action.openDisplaySettings': 'ディスプレイ設定を開く',
  'action.resetDisplayDimming': 'すべての減光を解除',
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
  'keyboard.pageDescription':
    '修飾キーの単押し・長押しの動作と、グローバルショートカットを設定します。',
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
  'keyboard.addRule': '修飾キールールを追加',
  'keyboard.editRule': '修飾キールールを編集',
  'keyboard.edit': '編集',
  'keyboard.editRuleFor': '修飾キールール「{label}」を編集',
  'keyboard.deleteRule': '修飾キールール「{label}」を削除',
  'keyboard.ruleSettings': '修飾キールールの設定',
  'keyboard.ruleLabel': '修飾キールールの名前',
  'keyboard.ruleEditorHint': '対象のキーと、長押し時の役割、単独で押したときの動作を選びます。',
  'keyboard.physicalKey': '対象のキー',
  'keyboard.keySide': 'キーの左右',
  'keyboard.noKeySide': '区別なし',
  'keyboard.holdRole': '長押し時の役割',
  'keyboard.keepOriginal': '元の修飾キーのまま使う',
  'keyboard.chooseHoldRole': '長押し時の役割を1つ選んで修復',
  'keyboard.reservedCommandHint':
    '左右の Command は入力切り替え用に予約されています。入力切り替えがオフでも個別には設定できません。「左右」を選ぶと、入力切り替えがオンの間はそちらが優先されます。',
  'keyboard.sendKeystroke': 'キーを送信',
  'keyboard.keystroke': '送信するキー',
  'keyboard.recordKeystroke': '送信するキーを記録',
  'keyboard.keystrokeHint':
    'キーを記録するか、Escape、Enter、Cmd+Space などを入力します。ここでは Escape も記録されます。記録を取り消すにはもう一度クリックするか、別の項目に移動します。',
  'keyboard.ruleEnabled': 'このルールを有効にする',
  'keyboard.saveRule': 'ルールを保存',
  'keyboard.savingRule': '保存中…',
  'keyboard.repairRule': '修復',
  'keyboard.repairRuleFor': '修飾キールール「{label}」を修復',
  'keyboard.createReplacement': '新しいルールを作成',
  'keyboard.createReplacementFor': '「{label}」の代わりに新しいルールを作成',
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
  'keyboard.configurationWarningTitle': '確認が必要なキーボード項目があります（{count} 件）',
  'keyboard.configurationWarningBody':
    '無効な保存済み項目は削除されていません。問題のないキーボード項目は引き続き動作します。',
  'keyboard.configurationWarningAction': 'キーボードで確認',
  'keyboard.configurationIssuesTitle': '一部の保存済みキーボード項目は動作していません',
  'keyboard.configurationIssuesBody':
    'キー入力を安全に保つため、以下の項目を読み飛ばしました。項目は保存されたままで、問題のない項目は引き続き動作します。',
  'keyboard.configurationIssuesHotkeys': 'ショートカット',
  'keyboard.configurationIssuesModifierRules': '修飾キー',
  'keyboard.deleteInvalidItem': '無効な項目「{label}」を削除',
  'keyboard.configurationIssueUnnamed': '名前のない項目（{id}）',
  'keyboard.configurationIssueUnknown': 'ID のない保存済み項目',
  'keyboard.configurationIssueReason.emptyId': '項目に ID がありません。',
  'keyboard.configurationIssueReason.idTooLong': '項目の ID が長すぎます。',
  'keyboard.configurationIssueReason.emptyLabel': '項目に名前がありません。',
  'keyboard.configurationIssueReason.labelTooLong': '項目の名前が長すぎます。',
  'keyboard.configurationIssueReason.invalidAccelerator':
    'ショートカットの形式が正しくありません。',
  'keyboard.configurationIssueReason.unsafeGlobalShortcut':
    '通常の文字入力を奪う可能性があるショートカットです。',
  'keyboard.configurationIssueReason.invalidKeystroke': 'キー送信アクションが正しくありません。',
  'keyboard.configurationIssueReason.reservedRuleId': 'Tomari が予約しているルール ID です。',
  'keyboard.configurationIssueReason.hyperWithRemap':
    'Hyper と別の長押しキーは同時に設定できません。',
  'keyboard.configurationIssueReason.reservedCommandSlot':
    '組み込みの入力切り替えが使用する Command キーです。',
  'keyboard.configurationIssueReason.duplicateId':
    '表記を正規化すると同じ ID になる保存済み項目が複数あります。',
  'keyboard.configurationIssueReason.duplicateAccelerator':
    '同じショートカットを使う保存済み項目が複数あります。',
  'keyboard.configurationIssueReason.duplicateModifierSlot':
    '同じ修飾キースロットを使う保存済みルールが複数あります。',
  'keyboard.configurationIssueReason.unsupportedModifierSide':
    'Caps Lock と fn に左右の区別はありません。「区別なし」を選んでください。',

  'recorder.click': 'クリックして記録',
  'recorder.typing': 'ショートカットを入力…',
  'recorder.typingKeystroke': '送信するキーを押してください…',
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
  'error.settingsRecoveryRequired': '設定を修復するまで変更を保存できません。',
  'error.databaseResetRequired':
    '隔離された設定データベースをリセットするまで Tomari を続行できません。',
  'error.backupInvalid':
    '対応する形式の正しい Tomari 設定バックアップではありません。別のファイルを選んでください。',
  'error.backupCurrentInvalid':
    '現在の設定を修復するまで、バックアップや置き換えはできません。「診断」と「キーボード」を確認してください。',
  'error.backupPreviewExpired':
    'プレビュー後に設定が変更されました。置き換える内容をもう一度確認してください。',
  'error.backupUnavailable': '復元できる以前の設定がありません。',

  'backup.title': '設定のバックアップ',
  'backup.privacy':
    '設定バックアップにはショートカットの割り当て、名前、アプリとディスプレイの識別子が含まれます。「診断」の匿名化されたサポート情報とは異なります。',
  'backup.exportTitle': '設定を書き出す',
  'backup.exportDescription':
    '一般設定、ショートカット、修飾キールール、記憶したウィンドウ位置、保存したディスプレイ構成を JSON ファイルに保存します。',
  'backup.export': '設定を書き出す',
  'backup.exporting': '書き出し中…',
  'backup.exported': '設定バックアップを保存しました。',
  'backup.openFolder': 'バックアップフォルダを開く',
  'backup.importTitle': 'ファイルから設定を復元する',
  'backup.importDescription':
    'Tomari の設定バックアップを選び、変更内容を確認してから現在の設定を置き換えます。',
  'backup.chooseFile': 'バックアップを選ぶ…',
  'backup.checking': 'バックアップを確認中…',
  'backup.restoreTitle': '最後の取り込み前の設定へ戻す',
  'backup.restoreDescription':
    '最後に取り込む直前の設定を確認して復元します。取り込み後に変更した設定も置き換わります。',
  'backup.restore': '取り込み前の設定へ戻す…',
  'backup.noRestore': '元に戻せる取り込みはありません。',
  'backup.statusFailed': '取り込み前の設定を確認できませんでした: {error}',
  'backup.imported': '設定を置き換えました。「取り込み前の設定へ戻す」で元に戻せます。',
  'backup.restored': '最後の取り込み前の設定に戻しました。',
  'backup.applyUnconfirmed':
    '処理結果を確認できなかったため、現在の設定を再読み込みしました。設定を確認し、必要であれば「取り込み前の設定へ戻す」を使用してください。',
  'backup.fileTooLarge': '2 MiB 以下の設定バックアップを選んでください。',
  'backup.busy': '設定の処理を完了しています…',
  'backup.confirmImportTitle': 'バックアップで設定をすべて置き換えますか？',
  'backup.confirmImportBody':
    '保存済みの設定をすべて置き換えます。件数が同じ項目も対象です。現在の設定は保存され、あとから戻せます。',
  'backup.confirmRestoreTitle': '最後の取り込み前の設定に戻しますか？',
  'backup.confirmRestoreBody':
    '取り込み後の編集を含め、設定をすべて置き換えます。この復元は1回だけ利用できます。現在の設定も残す場合は、先に書き出してください。',
  'backup.items': '保存済みの項目',
  'backup.current': '現在',
  'backup.replacement': '置き換え後',
  'backup.hotkeys': 'ショートカット',
  'backup.modifierRules': '修飾キールール',
  'backup.windowPlacements': 'ウィンドウ位置',
  'backup.displayProfiles': 'ディスプレイ構成',
  'backup.settingsChanges': '変更される一般設定',
  'backup.settingsUnchanged': '一般設定の値は同じです。上の保存済み項目は置き換わります。',
  'backup.setting': '設定項目',
  'backup.dragToSnap': 'ドラッグで整列',
  'backup.dragToMove': 'ドラッグで移動・リサイズ',
  'backup.runtimeExcluded':
    '権限、Caps Lock 設定の所有記録、実行中のスリープ防止は取り込み・復元しません。保存したディスプレイ構成は置き換えますが、現在の画面設定は変わりません。',
  'backup.hiddenIcon':
    'この設定ではメニューバーのアイコンが非表示になります。Spotlight から Tomari をもう一度起動すると設定を開けます。',
  'backup.reviewAgain': 'もう一度確認',
  'backup.replaceAll': '設定をすべて置き換える',
  'backup.applying': '設定を置き換え中…',

  'recovery.eyebrow': '安全インターロック',
  'recovery.title': '設定の修復が必要です',
  'recovery.intro':
    '保存された設定を安全に読み込めませんでした。読み取れない設定はまだ置き換えていません。',
  'recovery.databaseResetIntro':
    '破損した設定データベースを検出し、手動で復旧できるよう別の場所に保管しました。',
  'recovery.pausedTitle': '自動操作を停止しています',
  'recovery.pausedBody':
    'キー操作、ショートカット、ウィンドウ操作、メニューバーの自動操作は動作していません。',
  'recovery.optionsTitle': '続ける方法を選択',
  'recovery.optionsBody':
    '再読み込みは内容を変更しません。リセットすると、影響を受けた基本設定、ショートカット、修飾キールールが安全な初期値に戻る場合があります。読み取れるウィンドウ位置は、データベース自体を利用できない場合を除いて残ります。',
  'recovery.databaseResetOptionsTitle': 'リセットして続ける',
  'recovery.databaseResetOptionsBody':
    '破損したデータベースは安全に再読み込みできません。リセットを確認すると、自動操作を停止した初期値で代替データベースを初期化します。',
  'recovery.retry': 'もう一度読み込む',
  'recovery.retrying': '再読み込み中…',
  'recovery.reset': '設定をリセット…',
  'recovery.confirmTitle': '読み取れない設定をリセットしますか？',
  'recovery.confirmBody':
    '影響を受けたデータを安全な初期値に戻します。ショートカットやルールも置き換わる場合があり、読み取れるウィンドウ位置はデータベース全体の隔離が必要でない限り残ります。各機能を再びオンにするまで自動操作は停止したままです。',
  'recovery.confirmAction': 'リセットして続ける',
  'recovery.resetting': 'リセット中…',
  'recovery.retryFailed': '設定をまだ読み込めませんでした: {error}',
  'recovery.resetFailed': '設定をリセットできませんでした: {error}',

  'setup.title': 'Tomari を使う準備',
  'setup.intro':
    'ウィンドウの操作とキー操作の認識には、macOS の権限を 2 つ使います。ディスプレイ管理には不要です。「あとで設定する」を選ぶと、権限を許可せずに利用できます。',
  'setup.accessibility': 'アクセシビリティ',
  'setup.accessibilityWhy':
    'ウィンドウの移動・リサイズ、キー送信、メニューバー項目の読み取り・並べ替えに使います。',
  'setup.inputMonitoring': '入力監視',
  'setup.inputMonitoringWhy':
    '修飾キーの単押し・長押し、Hyper キー、ウィンドウのドラッグ操作を認識するために使います。',
  'setup.granted': '付与済み',
  'setup.grant': 'システム設定を開く',
  'setup.grantFor': '{name} のシステム設定を開く',
  'setup.requesting': '開いています…',
  'setup.openAgain': 'もう一度開く',
  'setup.returnHint': 'システム設定で許可したら、Tomari に戻ってください。',
  'setup.requestedHint':
    '許可しても機能しない場合は Tomari を再起動してください。アプリを更新した後も未許可のままなら、システム設定のスイッチがオンでも古い Tomari を削除し、今のアプリを追加し直してください。',
  'setup.later': 'あとで設定する',
  'setup.done': 'Tomari を使い始める',
  'setup.allSet': '準備完了です。Tomari を使い始められます。',
  'setup.tryIt': '試してみる: {keys} で直前まで使っていたウィンドウが左半分にスナップします',
  'setup.bannerText.one': 'macOS の権限があと 1 つ必要です',
  'setup.bannerText.two': 'macOS の権限が 2 つ必要です',
  'setup.updateBannerText': 'アップデート後の権限を確認してください',
  'setup.bannerDescription.both': '許可するまでは、ウィンドウ操作とキー操作の一部が制限されます。',
  'setup.bannerDescription.accessibility':
    '許可するまでは、ウィンドウ操作、キー送信、メニューバー項目の読み取り・並べ替えが制限されます。',
  'setup.bannerDescription.inputMonitoring':
    '許可するまでは、修飾キー操作とウィンドウのドラッグ機能が制限されます。',
  'setup.bannerAction': '権限を確認',
  'setup.openSetup': 'セットアップを開く',
  'setup.updateRegrant':
    'アップデート後に権限が外れてしまったようです（現時点の Tomari の既知の制約です）。もう一度許可してください。',
  'setup.adminNote':
    '蓋を開いた通常のスリープ防止には権限の許可は不要です。蓋を閉じても継続するモードで、蓋閉じ時の設定を変更するときだけ管理者パスワードを確認します。',

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
  'window.homeDamaged': '保存した位置が壊れていて使えません。上書きするか削除してください。',
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
  'menubar.unsupportedOs':
    'macOS 27 では、アプリがステータスアイコンをメニューバーの外へ押し出せなくなりました。' +
    'このためメニューバー整理は使えません。アイコンが収まりきらないときは、' +
    'macOS がアイコンの横に出す展開ボタンを使ってください。',
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
    'ダウンロードやビルドなどの長い処理を続けます。MacBook の蓋を閉じても継続するか選べます。一時的な設定で、Tomari の起動時に自動で再開しません。',
  'settings.keepAwakeMode': 'スリープを防ぐ範囲',
  'settings.keepAwakeIdle': '蓋を開いている間',
  'settings.keepAwakeLid': '蓋を閉じても継続',
  'settings.keepAwakeIdleHint':
    '管理者認証なしで、操作していない間の自動スリープを防ぎます。蓋を閉じたときや手動でのスリープは通常どおり動作します。範囲を変更するには一度停止してください。',
  'settings.keepAwakeLidHint':
    'MacBook の蓋を閉じたときもスリープを防ぎます。開始・停止で蓋閉じ時の設定を変更するときは管理者認証が必要です。範囲を変更するには一度停止してください。',
  'settings.keepAwakeModeRecoveryHint':
    '前回の蓋閉じ時の設定を確認してから、範囲を変更できます。設定を元に戻すには管理者認証が必要です。',
  'settings.keepAwakeAction': 'この Mac のスリープを防ぐ',
  'settings.keepAwakeStart': 'スリープ防止を開始',
  'settings.keepAwakeStartAdmin': 'スリープ防止を開始…',
  'settings.keepAwakeStop': 'スリープ防止を停止',
  'settings.keepAwakeStopAdmin': 'スリープ防止を停止…',
  'settings.keepAwakeActive': 'スリープを防いでいます',
  'settings.keepAwakeInactive': '通常どおりスリープします',
  'settings.keepAwakeEnabling': 'オンにしています…',
  'settings.keepAwakeDisabling': 'オフにしています…',
  'settings.keepAwakeEnablingAdmin': 'オンにしています。管理者の承認を待っています。',
  'settings.keepAwakeDisablingAdmin': 'オフにしています。管理者の承認を待っています。',
  'settings.keepAwakeFailed': '直前の変更を適用できませんでした',
  'settings.lidClose': '蓋を閉じても継続',
  'settings.lidActive': '有効',
  'settings.lidPending': '有効化中…',
  'settings.lidUnavailable': '利用不可',
  'settings.lidOff': 'オフ',
  'settings.authorizationPendingHint':
    'macOS のダイアログを完了するか、キャンセルして直前の状態に戻してください。',
  'settings.safetyGuards': '自動で解除する条件',
  'settings.autoOff': '時間が経ったら',
  'settings.autoOffHint':
    '指定した時間が経つと、再びスリープできるようにします。この画面を閉じていても Tomari が見守ります。',
  'settings.autoOffNever': '解除しない',
  'settings.autoOff30m': '30分後',
  'settings.autoOff1h': '1時間後',
  'settings.autoOffAtTime': '時刻を指定…',
  'settings.endTime': '解除する時刻',
  'settings.timeRemaining': '残り時間',
  'settings.acOnly': '電源アダプタを外したら',
  'settings.acOnlyHint':
    'バッテリー駆動になった時点で、再びスリープできるようにします。オンにするときも電源接続が必要です。',
  'settings.lowBattery': 'バッテリー残量が20%になったら',
  'settings.lowBatteryHint': 'バッテリー残量が20%以下になったときの動作です。',
  'settings.lowBatteryWarn': '警告のみ',
  'settings.lowBatteryTurnOff': 'スリープ防止を解除',
  'settings.systemState': 'システム状態',
  'settings.powerSource': '電源',
  'settings.powerAc': '電源アダプタ',
  'settings.powerBattery': 'バッテリー',
  'settings.systemUnknown': '不明',
  'settings.advancedState': '詳細',
  'settings.kernelState': 'macOS のスリープ禁止フラグ',
  'settings.kernelStateHint':
    'macOS から直接読み取った SleepDisabled の値です。通常の自動スリープ防止では、この蓋閉じ時の設定は変更しません。',
  'settings.kernelBlocked': '蓋閉じ時のスリープ禁止',
  'settings.kernelAllowed': '蓋閉じ時のスリープ可能',
  'settings.ownership': '蓋を閉じたときの設定の変更元',
  'settings.ownershipHint': 'Tomari は、自分で変更した蓋閉じ時の設定だけを元に戻します。',
  'settings.ownedByTomari': 'Tomari',
  'settings.notOwned': 'Tomari は変更していない',
  'settings.detectedJobs': '長時間実行中のジョブ',
  'settings.detectedJobsHint': '5分以上動作している開発プロセスです。',
  'settings.noDetectedJobs': '長時間実行中の開発ジョブは見つかりませんでした。',
  'settings.processDetail': 'PID {pid} · 実行時間 {elapsed}',
  'settings.noticeAcRequired': '電源アダプタを接続してからオンにしてください。',
  'settings.noticeAcDisconnected': '電源アダプタが外れたため、スリープ防止を解除しました。',
  'settings.noticeLowBattery': 'バッテリー残量が20%以下です。',
  'settings.noticeTimerElapsed': '指定した時間が経ったため、スリープ防止を解除しました。',
  'settings.noticeAuthorizationDeclined':
    '管理者による変更が拒否されたか、結果を確認できませんでした。',
  'settings.noticeLidCloseUnconfirmed':
    'ふたを閉じたときのスリープ設定を確認できず、まだ有効なままかもしれません。再試行すると元に戻します。',
  'settings.noticeAssertionFailed':
    'macOS にスリープ防止の変更を適用できませんでした。現在の状態を確認して再試行してください。',
  'settings.noticeLeftoverOverride':
    'この Mac では現在、ふたを閉じてもスリープしない設定になっています。前回の Tomari が残した可能性がありますが、その後にあなたや別のアプリが設定した可能性もあります。スリープを元に戻すか、このままにするかを選んでください。',
  'settings.leftoverClear': 'スリープを元に戻す',
  'settings.leftoverKeep': 'このままにする',

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

  'app.restart': 'Tomari を再起動',
  'app.restarting': 'Tomari を再起動しています…',
  'app.restartFailed': 'Tomari を再起動できませんでした: {error}',
  'app.restartHint':
    'Tomari を再起動すると、これらの機能の開始をもう一度試します。改善しない場合は診断を開いてください。',
  'settings.openPermissionSettings': '権限の設定を開く',
  'settings.openDiagnostics': '診断を開く',
  'settings.applyWarning.launchAtLogin':
    'Tomari の自動起動を変更できませんでした。システム設定の「ログイン項目」を確認してください。',
  'settings.applyWarning.menuBar':
    'メニューバーの整理が動作していません。診断で、この macOS が対応しているか確認できます。',
  'settings.applyWarning.keyboardTap':
    'キーボードのカスタマイズが動作していません。修飾キーのタップ・長押し・割り当て変更が使えない可能性があります。',
  'settings.applyWarning.globalShortcuts':
    '一部のショートカットが使えません。ほかのアプリとキーが重複している可能性があります。キーボード設定の「ショートカット」を確認してください。',
  'settings.applyWarning.dragToSnapTap':
    'ドラッグでスナップが動作していません。ウィンドウを画面端にドラッグしても整列しません。',
  'settings.applyWarning.dragToMoveTap':
    'ドラッグで移動が動作していません。修飾キーを押しながらのドラッグでウィンドウを移動・サイズ変更できません。',
  'settings.applyWarning.capsLockRemap':
    'Caps Lock の割り当て変更が使えません。以前の動作のままになっている可能性があります。',
  'settings.applyWarning.commandImeRules':
    'Command キーでの入力切り替えが動作していません。Command をタップしても入力ソースが変わらない可能性があります。',
  'settings.applyWarning.generic':
    'Tomari の一部の機能を開始できませんでした。診断で利用できる機能を確認してください。',

  'health.title': '診断',
  'health.pageDescription':
    'Tomari の各機能が正しく動作しているか、必要な macOS の権限がそろっているかを確認します。',
  'health.reading': 'システムの状態を確認しています…',
  'health.loadFailed': '診断情報を読み取れませんでした: {error}',
  'health.allReady': '問題は検出されませんでした',
  'health.needsAttention': '{count} 件の対応が必要です',
  'health.needsAttentionOne': '1 件の対応が必要です',
  'health.pendingSummary': '{count} 件を処理しています',
  'health.pendingSummaryOne': '1 件を処理しています',
  'health.snapshotMeta': 'Tomari {version} · {architecture} · 今確認した状態',
  'health.refreshFailed': '更新に失敗したため、直前の状態を表示しています: {error}',
  'health.refreshing': '更新中…',
  'health.tone.ready': '正常',
  'health.tone.attention': '対応が必要',
  'health.tone.pending': '処理中',
  'health.tone.inactive': 'オフ',
  'health.group.input': 'キーボードとマウス',
  'health.group.system': 'システム',
  'health.group.support': 'サポート',
  'health.permissions': 'macOS の権限',
  'health.permissionsGranted': 'アクセシビリティと入力監視は許可されています。',
  'health.permissionsMissingBoth':
    'アクセシビリティと入力監視が許可されていません。セットアップから許可してください。',
  'health.permissionsMissingAccessibility':
    'アクセシビリティが許可されていません。セットアップからシステム設定を開いてください。',
  'health.permissionsMissingInputMonitoring':
    '入力監視が許可されていません。セットアップからシステム設定を開いてください。',
  'health.tap.keyboard': 'キーボードの監視',
  'health.tap.dragToSnap': 'ドラッグスナップの監視',
  'health.tap.dragToMove': 'ドラッグ移動の監視',
  'health.tapOff': '設定でオフになっています。',
  'health.tapRunning': '動作しています。',
  'health.tapCounters':
    '再開 {restarts} 回 · macOS による停止 {disables} 回 · 復旧 {recoveries} 回',
  'health.tapState.stopped': '機能はオンですが、Tomari が入力を受け取れていません。',
  'health.tapState.starting': '起動しています。少し待ってから更新してください。',
  'health.tapState.disabledByTimeout':
    'macOS により入力の監視が停止されました。Tomari が再開しています。',
  'health.tapState.permissionDenied':
    '入力監視またはアクセシビリティが許可されていません。セットアップから許可してください。',
  'health.tapState.failed':
    '入力の監視を開始できませんでした。設定で機能をオフにしてから、もう一度オンにしてください。',
  'health.capsLock': 'Caps Lock の割り当て',
  'health.capsHeld': 'Caps Lock の割り当ては有効で、正しく動作しています。',
  'health.capsOff': 'Caps Lock の割り当ては変更されていません。',
  'health.capsPending': 'Caps Lock の割り当てを反映しています。少し待ってから更新してください。',
  'health.capsUnknown':
    'Caps Lock を現在どのアプリが制御しているか確認できません。修飾キー設定を開いてください。',
  'health.capsMismatch': 'Caps Lock が設定どおりに動作していません。修飾キー設定を開いてください。',
  'health.shortcuts': 'グローバルショートカット',
  'health.shortcutsOff': 'キーボードのカスタマイズはオフです。',
  'health.shortcutsRegistered': '{registered} 件のショートカットを登録しています。',
  'health.shortcutsRegisteredOne': '1 件のショートカットを登録しています。',
  'health.shortcutsInvalid':
    '保存済みショートカットのうち {invalid} 件を読み込めませんでした。ショートカット設定で確認してください。',
  'health.shortcutsConflict':
    '一部のショートカットを登録できませんでした。他のアプリとキーが競合している可能性があります（{registered} 件は有効）。',
  'health.menuBar': 'メニューバーの整理',
  'health.menuBarOff': 'メニューバーの整理はオフです。',
  'health.menuBarReady': 'アクセシビリティは許可され、メニューバーの区切りも配置されています。',
  'health.menuBarUnsupported':
    'この環境ではメニューバーの整理機能を利用できないため、オフのままです。',
  'health.menuBarPermissionMissing':
    'メニューバーを整理するにはアクセシビリティ権限が必要です。セットアップから許可してください。',
  'health.menuBarDividerMissing':
    'Tomari のメニューバー区切りを確認できません。メニューバー設定から復元してください。',
  'health.keepAwake': 'スリープ防止',
  'health.keepAwake.off': 'スリープ防止はオフです。',
  'health.keepAwake.on': 'スリープ防止はオンで、Mac はスリープしません。',
  'health.keepAwake.idleOn':
    '操作していない間の自動スリープを防いでいます。蓋を閉じたときや手動でのスリープは通常どおり動作します。',
  'health.keepAwake.enabling': 'スリープ防止を有効にしています。完了後に更新してください。',
  'health.keepAwake.disabling': 'スリープ防止を無効にしています。完了後に更新してください。',
  'health.keepAwake.failed': 'スリープ防止を適用できませんでした。設定を開いて再試行してください。',
  'health.keepAwake.residual':
    'スリープ防止はオフですが、Mac がスリープしない状態のまま残っている可能性があります。スリープ防止設定を開いてください。',
  'health.keepAwake.mismatch':
    'スリープ防止が設定どおりに動作していません。設定を開いて再適用してください。',
  'health.database': '保存されたデータ',
  'health.databaseReady': '保存されたデータは最新の形式で、破損もありません。',
  'health.databaseFailed':
    '保存されたデータが破損している可能性があります。サポートバンドルを出力してください。',
  'health.databaseOutdated':
    '保存されたデータが、このバージョンより古い形式のままです。サポートバンドルを出力してください。',
  'health.databaseUnavailable':
    '保存されたデータの状態を確認できませんでした。調査用のサポートバンドルを出力してください。',
  'health.updates': 'アップデートの検証',
  'health.updateSignatureReady': 'アップデートはインストール前に、正規のものかどうか検証されます。',
  'health.updateSignatureMissing':
    'このビルドではアップデートが正規のものかどうか検証できません。サポートバンドルを出力してください。',
  'health.action.permissions': 'セットアップを開く',
  'health.action.modifiers': '修飾キー設定を開く',
  'health.action.windowMouse': 'マウス設定を開く',
  'health.action.shortcuts': 'ショートカットを開く',
  'health.action.menuBar': 'メニューバー設定を開く',
  'health.action.keepAwake': 'スリープ防止を開く',
  'health.supportBundle': 'サポートバンドル',
  'health.supportBundleDescription':
    'この診断結果を、サポートに共有できるファイルとして保存します。入力した内容、ショートカット設定、メニューバーの項目名、保存されたデータ、エラーの詳細、ファイルパスは含みません。',
  'health.export': 'サポートバンドルを出力',
  'health.exporting': '出力中…',
  'health.exported': 'サポートバンドルを保存しました',
  'health.exportFailed': 'サポートバンドルを出力できませんでした: {error}',
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
