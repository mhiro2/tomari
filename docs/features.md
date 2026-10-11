# Feature reference

A detailed look at everything Tomari can do. For a quick overview, see the
[README](../README.md).

## Diagnostics

The **Diagnostics** section reads a fresh, read-only snapshot of every runtime
boundary that can leave stored settings out of step with macOS: Accessibility
and Input Monitoring, each event tap's state and restart/disable/recovery
counters, the Caps Lock HID claim, global shortcut registration, Menu Bar
Accessibility availability, Prevent Sleep's marker and kernel state, SQLite
integrity and migration status, and updater signature configuration. Features that
are deliberately switched off are shown as inactive rather than failed.
Actionable failures explain the likely cause and link directly to the relevant
Setup dialog or settings tab. Transient startup and shutdown states are shown
as in progress and do not ask the user to intervene.

Diagnostics never performs a Menu Bar Accessibility inventory scan. It reads
only cached divider and permission state, leaving item IDs and the visible menu
bar untouched; full inventory refreshes happen only from the Menu Bar settings
screen.

**Export Support Bundle** writes the same snapshot as versioned JSON under
Tomari's per-user data directory. The bundle contains state names, booleans,
versions, and aggregate counts only. It never includes key or pointer events,
shortcut accelerators or labels, Accessibility labels, process names or PIDs,
configured bindings or actions, database rows, error strings, or filesystem
paths.

## Keyboard

The **Keyboard** settings separate the two kinds of configuration into
**Modifier Keys** and **Shortcuts** tabs. Modifier Keys presents each physical
key as a row with its tap action, held role, and enabled state; the left/right
Command IME switch is shown as a direct key-to-input-source mapping. Global
shortcut recording stays in the Shortcuts tab so it does not compete with the
modifier table.

Use **Add Modifier Rule** or a row's **Edit** button to configure its label,
physical key, side, held role, and tap action. The held role is either the
original modifier, another modifier, or Hyper; choosing one never combines
Hyper with a separate remap. Caps Lock and Fn use Either because those keys
have no left/right pair. Left and right Command remain reserved for the IME
switch even while that switch is off; an Either-side Command rule is separate
and side-specific rules take precedence.

The editor supports IME actions, window actions, and **Send a keystroke** as tap
actions. The keystroke recorder accepts single keys such as Escape, Enter, or a
letter, while global-shortcut recording retains its modifier requirement.
Changes stay in the editor until **Save Rule** succeeds. Cancel discards the draft;
deleting an existing rule requires a second confirmation in the editor.

- **Modifier remapping** — remap Caps Lock / Control / Option / Command /
  Shift / fn to another modifier (for example, _Caps Lock → Control_).
- **Tap vs. hold** — tap a modifier on its own, quickly, to fire a dedicated
  action; keep holding it and it behaves as the normal modifier.
- **IME switching with left/right ⌘** — tap the left ⌘ to switch to English
  (Eisū), tap the right ⌘ to switch to Japanese (Kana). Both halves are governed
  by a single on/off switch.
- **Tap actions** — a modifier tap can show/hide the panel, snap a window,
  switch the IME, or send an arbitrary key.
- **Hyper key** — while a modifier is held, fire ⌃⌥⇧⌘ together. This gives you
  a dedicated hotkey range that rarely collides with existing shortcuts.
- **Global shortcuts** — bind any action to an accelerator (for example,
  `⌃⌥←`). In the settings UI you record a shortcut by clicking the field and
  simply pressing the keys you want; recorded chords are shown with the native
  macOS glyphs (⌃ ⌥ ⇧ ⌘ and ← ↑ → ↓), not `Ctrl`/`Alt` legends.

### Invalid saved keyboard items

Tomari checks every saved shortcut and modifier rule at launch, whenever the
keyboard configuration is reloaded, and before a changed item is saved. This
also protects against rows created by an older Tomari release or by editing the
database directly. Invalid items never become active, but they are not deleted:
the Keyboard page identifies them, Tomari leaves their stored values untouched,
and all valid items keep working.

The checks reject malformed key combinations and keystroke actions, ordinary
typing keys used as global shortcuts without Control, Option, or Command,
invalid names or identifiers, Hyper combined with a separate remap, and slots
reserved for the built-in left/right Command IME switch. A Caps Lock or Fn rule
with a left/right side is also invalid. If multiple rows normalize to the same
identifier, shortcut, or physical modifier slot, Tomari skips every row in that
group rather than choosing one based on database order.
Disabled rows take part too, preventing an item from silently displacing another
when it is enabled later. The built-in Command IME pair is added only after the
saved rules pass these checks.

A persistent amber notice reports how many saved keyboard items need attention
and links to localized details on the Keyboard page. The notice remains until
the affected rows are repaired or deleted. Readable modifier rules offer
**Repair**, opening their values in the editor; invalid identities can be
replaced without leaving a second copy of the original row. A rule that cannot
be reconstructed instead offers a path to create a new rule. Every warning
also offers deletion by its exact stored identity, including window shortcuts
and modifier rules that the editor cannot reconstruct. Persisted labels and
accelerators are bounded and stripped of control characters for display without
changing the stored values used by repair or deletion. This is separate from an apply warning,
which means a valid setting reached macOS but its live system change failed.

### Notes on remapping

Remapping rewrites event flags and key codes at the event-tap level. Control,
Option, Command, Shift, and fn can be tracked per side for both press and
release, so they remap correctly as momentary modifiers (active only while
held). Their remapped or Hyper flags apply to clicks, drags, and scrolling as
well as keystrokes; using the pointer also prevents a held modifier from being
mistaken for a solo tap action.

Caps Lock is special. macOS delivers it as a _lock_ — one event per press, no
key release, and the upper-case lock applied below the event tap — so the event
tap alone can neither time a hold nor stop it locking. To make it usable as a
modifier, Tomari remaps the Caps Lock key to **F18** (an unused ordinary key) at
the HID level using the same `UserKeyMapping` facility as macOS's `hidutil` tool.
The remap happens before the lock is applied, so Caps Lock no longer locks, and
F18 behaves as an ordinary key with real press/release — which Tomari then
handles as the Caps Lock modifier, acting as the remapped modifier (Control by
default) whether tapped or held.

This needs no extra setup: the remap is applied automatically while a Caps Lock
rule is enabled and removed when Tomari quits or the rule is turned off. A few
consequences to be aware of:

- It merges with, rather than replaces, any custom `hidutil` key mappings you
  have set yourself: Tomari adds (and later removes) only its own Caps Lock → F18
  entry, leaving your other mappings in place. If Tomari cannot read your current
  mappings back exactly, it does not write at all — the Caps Lock rule is
  reported as not applied rather than risking your mappings.
- If you had mapped the Caps Lock key itself to something else, Tomari has to
  take that key over — so it remembers what you had it doing and puts it back
  when the rule is turned off or Tomari quits. If that cannot be remembered, the
  remap is not applied at all rather than losing your mapping. And if you had
  already mapped Caps Lock to F18 yourself, Tomari uses it as-is and never
  removes it — only a mapping Tomari made is one Tomari takes back.
- While a Caps Lock rule is active, a physical **F18** key (uncommon) is treated
  as Caps Lock too — the remap makes them indistinguishable.
- If Tomari is force-quit (rather than quit normally) the remap can persist until
  the next launch, which removes it again. In the rare case where Tomari cannot
  tell whether a remap is its own — it lost track of it mid-change — it leaves it
  alone rather than risk removing one of yours. Either way you can clear just
  that entry yourself by running `hidutil property --get UserKeyMapping`,
  removing the Caps Lock → F18 entry, and setting the rest back.

## Window management

The **Windows** settings are split into **Saved Positions**, **Shortcuts**, and
**Mouse** tabs. Saved Positions keeps the focused application, its current
frame, and Position A/B together; Shortcuts puts each window action beside its
recorded chord; Mouse gives drag-to-snap and drag-to-move/resize their own
visual controls.

- **Two remembered positions per app** — place a focused app where it belongs,
  then save that position as Position A or Position B in the Windows section.
  Replacing a position can be undone from the confirmation toast; forgetting
  one requires a second click and can also be undone. Tomari identifies the app
  by bundle id and never stores the window title.
- **Display-safe restore** — homes are stored as position and size relative to
  the usable display rather than as desktop pixels. Restoring applies the same
  proportion to the current display, including after a display is disconnected,
  reconnected, resized, or replaced.
- **Restore from the working context** — use `⌃⌥↓` by default, bind the action
  to another global shortcut, or assign Restore as a modifier-key tap under
  Keyboard. Caps Lock → Control users can choose it as an optional accelerator:
  a tap restores the window and a hold remains Control. Repeating restore on
  the same unmoved window alternates Position A and Position B. The panel
  refreshes its focused-app context whenever it is shown, and each button
  verifies the exact represented window before applying an action.
- **Move and restore** — `⌃⌥⇧→` moves the focused window to the next display and
  applies Position A there as one action. Position B is used when Position A is
  absent; all window shortcuts are configured beside the workflow in the
  Windows section.
- **Undo and redo** — `⌃⌥Z` and `⌃⌥⇧Z` reverse or reapply window changes,
  including remembered-home restores, display moves, preset snaps, and drag
  snaps. The tray names these actions explicitly as window changes so their
  scope is clear outside the Windows screen, and enables them only when the
  matching history is available.
- **Quick tiling remains available** — `⌃⌥←` / `⌃⌥→` / `⌃⌥↑` snap to the left
  half, right half, and maximize. Repeating a half shortcut cycles
  1/2 → 1/3 → 2/3. All 15 presets, ordinary next/previous-display moves, and
  move-and-restore actions can be added as shortcuts, while the main UI stays
  centered on the focused app rather than a 15-zone palette.
- **Drag-to-snap (optional)** — drag a window to a screen edge or corner to
  show a preview, then release to snap to a half, a corner, or full screen
  depending on where you let go.
- **Drag-to-move & resize (optional)** — hold `⌃⌥` and drag anywhere inside a
  window to move it, or `⌃⌥⌘` to resize it from the bottom-right (the top-left
  corner stays anchored). It acts on the window under the pointer with no need
  to click it first, and while a gesture is held the drag is consumed so the app
  underneath never sees it — including when there is no window to drag under the
  pointer, since holding the chord is taken as meaning the click is for Tomari.
Opening Tomari from the menu bar temporarily gives the panel focus. The Windows
section deliberately resolves the frontmost other application, so Remember,
Restore, and the preview continue to target the app you were using rather than
Tomari's own settings window.

## Menu bar tidying

Menu bar tidying is unavailable on the supported macOS 27+ baseline. The
previous divider mechanism cannot hide status icons there. The Menu Bar page
shows an explanation, Diagnostics reports the feature as unsupported, and no
divider or handle is created. Tomari leaves the system's overflow control in
charge.

Keyboard settings do not offer the menu bar action. A previously saved shortcut
for that action remains inactive and does not reserve its keys. Other menu bar
app behavior, including Tomari's tray menu, is unaffected.

## Prevent Sleep (keep awake)

Keep long-running jobs from AI agents (Codex, Claude Code, and the like) from
being interrupted — **even when the display is closed**. You can toggle it
manually from the menu bar tray, the Start / Stop Preventing Sleep button in the
Prevent Sleep section, or a global shortcut (the "Toggle Prevent Sleep" action). Settings also lists common
developer processes such as Codex, Claude, Cargo, Node, and Python once they
have run for five minutes, so you can see at a glance whether something worth
protecting is still running. The list is informational only: it never enables
the feature by itself, and the switch above it is the one place to turn it on.

Choose a mode before starting:

- **Lid is open** is the default. An IOKit power assertion
  (`PreventUserIdleSystemSleep`) prevents idle system sleep without administrator
  authorization. It does not prevent lid-close sleep or change the system-wide
  `disablesleep` setting. The display can still sleep.
- **Lid is open or closed** adds `pmset disablesleep` to that
  assertion. Starting and stopping this mode require your administrator password.
  Declining a start cancels that operation without switching modes;
  declining a stop leaves the session on until the override is cleared.

The mode can change only while stopped, with no pending transition or unresolved
Tomari-owned sleep override. Timer and power policies can still change during a
session. The selection is session-only, returns to lid-open mode after relaunch, and
is not included in configuration backups.

The UI models authorization operations explicitly as **enabling** and **disabling**.
Every toggle surface is locked for the full lifetime of the administrator
prompt—not merely until the backend worker starts. Canceling from Settings
safely drives the kernel back toward the preceding stable state; a
failed authorization can be retried without guessing which direction failed.

Session-only safety controls can bound the operation:

- turn it off after 30 minutes, after one hour, or at a selected date and time;
- allow it only while connected to AC power;
- warn at 20% battery, or begin turning it off automatically;
- follow the remaining time in a live countdown.

The 30-minute and one-hour presets start counting when Prevent Sleep actually
turns on, so they work the same from the tray or a shortcut. A selected end time
that has already passed is treated as spent: turning Prevent Sleep on clears it
and starts an unbounded session rather than refusing, and Settings drops the
stale time from the picker. Pick a new end time to bound the new session.

Automatic turn-off uses the selected mode's manual stop path. Lid-open mode releases
its assertion without authorization; lid-closed mode requests administrator
approval to clear `disablesleep`. Because you can decline that approval, a guard
attempts to turn Prevent Sleep off at most once per session rather
than reopening the password prompt every few seconds; the Prevent Sleep page
then offers Retry next to the main button (which stays disabled until the retry
settles). The safety
monitor runs in the backend, including while Settings is closed: every ten
seconds while Prevent Sleep is on or the window is open, reacting at once when
the Mac is plugged in or unplugged, and only every couple of minutes otherwise so
an idle Tomari stays quiet. The System state section separately reports the real kernel
`SleepDisabled` flag and whether Tomari owns the override, rather than inferring
either value from the switch.

The state lasts **only for the current session**: Tomari always starts with
Prevent Sleep off. The idle assertion is released at exit and by macOS if the
process exits unexpectedly. Lid-closed cleanup also clears Tomari's owned
`disablesleep` override. If that mode crashed while it was on, the next launch
finds the failsafe record it left and shows a
notice in the Session view — but it does not clear the setting on its own,
because by then the setting could just as well have been turned on by you or by
another app. You choose: **Turn sleep back on** clears it (an administrator
prompt appears), **Leave it as it is** keeps it and forgets the record. Until
you choose, Prevent Sleep cannot be started from the panel, the tray or a
hotkey, and quitting leaves the setting untouched so the question is asked
again next time.

Running with the lid closed increases battery drain and heat, so keeping the
machine plugged in is recommended.

## Menu bar and window

Tomari runs as a menu bar app: clicking its menu bar icon opens a compact menu
for permission recovery, undo/redo of window changes, Prevent Sleep, menu-bar
icons, and Settings. Placement choices live with their app context in the
Windows section or on shortcuts rather than in a generic tray palette. The
settings window has no overview page: its sidebar contains the five direct
destinations **Windows**, **Keyboard**, **Menu Bar**, **Prevent Sleep**, and
**General**, grouped under Tools and App. Sidebar rows use the destination name
instead of repeating descriptions or feature-state badges. Reopening Settings
returns to the last selected destination (or Windows when no valid selection has
been saved).

Each feature page starts with one short purpose sentence and, where applicable,
a master control as the first row of the page. For Windows, Keyboard, and Menu
Bar it is a switch; turning the feature off leaves its settings visible so its
scope is still understandable, but disables the controls until the feature is
turned back on. Prevent Sleep is an operation rather than a preference, so its
first row offers Start Preventing Sleep… / Stop Preventing Sleep buttons instead,
and its auto-off conditions stay editable while off. Windows,
Keyboard, and Menu Bar use the focused tabs described above; ordinary options
use divided rows, while cards are reserved for the master control and for
objects such as saved window positions.

On the very first launch the settings window opens automatically (the main
features need permissions you have not granted yet); after that Tomari starts
silently in the menu bar. If Tomari cannot establish one trustworthy settings
snapshot, it opens a dedicated recovery screen instead of the ordinary feature
pages. Keyboard gestures, shortcuts, window controls, and menu-bar automation
remain off, and ordinary settings writes are blocked. **Try Again** only
re-reads the saved configuration. **Reset Settings** requires a second
confirmation, replaces the unreadable startup configuration with a safe
automation-off profile, and relaunches Tomari; affected shortcut or modifier
rows may return to defaults, while readable saved window positions are kept.
A physically damaged database is retained beside the replacement with a
`.broken-<timestamp>` suffix for manual recovery. Its reset requirement
survives quitting or a crash, so an empty replacement is never mistaken for a
first launch. Because retrying cannot make that empty replacement trustworthy,
this case offers only the confirmed reset action. After a successful retry or
reset, the settings panel opens again automatically. Reset leaves automation
off until you re-enable it, while Try Again preserves the repaired saved
settings.

A semantically invalid but readable shortcut or modifier rule does not require
this process-wide recovery screen. Tomari quarantines only the affected keyboard
items, shows the amber notice described above, and keeps the rest of the valid
configuration running.

On the first launch, or after an update invalidates a previously granted
permission, Settings opens with a focused Setup dialog over the current page
(see [Permissions](permissions.md)). Dismissing it returns to the settings pages.
Afterward, permission health is summarized once in the sidebar footer: a green
ready status when both permissions are granted, or **Needs attention** when
either is missing. Selecting **Needs attention** reopens Setup; individual
feature pages do not repeat permission banners.

You can hide the icon with **Show in menu bar** in the General section if you
prefer a fully background app. Because hiding it removes the app's only visible
affordance (Tomari has no Dock icon), turning it off asks you to confirm first
and spells out how to reopen the window.

Even with the icon hidden, you can always reopen the window:

- **Launch Tomari again** from Spotlight or Launchpad. Tomari runs as a single
  instance, so a second launch surfaces the window instead of starting a copy.
  This works regardless of how your shortcuts are configured, so it is the
  reliable recovery path if you have changed or removed the default shortcut.
- Use the **global shortcut** bound to the "Show/hide Tomari" action (default
  ⌘⇧K).
- Call **`tomari://v1/toggle-panel`** (see the [URL scheme](url-scheme.md)). This
  always works, even when external window control is turned off.

## Settings backups

Under **General → Settings backup**, **Export Settings** creates a versioned
JSON backup of general preferences, global shortcuts, modifier rules, every
app's saved Position A/B, and saved display configurations. It writes a local file in Tomari's application-data
`backups` folder; **Open Backup Folder** opens that location in Finder. Files
are created with owner-only read/write permissions.

A backup contains your labels, shortcut bindings, keystroke actions, application
names and bundle identifiers, saved positions, and display UUIDs and mode descriptions. It is intended to restore
your configuration and is not an anonymized support report. Keep it private
when sharing diagnostic information. It does not contain raw input, window
titles, macOS permission grants, HID ownership records, Prevent Sleep's active
session or safety policies, or window-movement history. Importing never grants
permissions or starts a Prevent Sleep session.

Use **Choose Backup…** to preview a JSON file's contents, then confirm with
**Replace All Settings**. Import **replaces all five configuration groups**; it
does not merge rows with the existing configuration. Canceling the preview changes
nothing. Unsupported format versions, incomplete or unknown fields, invalid
values, and conflicting keyboard records are rejected before anything is
written. Version 2 allows at most 2 MiB, 1,000 records per keyboard or window
collection, and 32 display profiles. Version 1 files are not accepted. The schema
upgrade clears the old, incompatible one-step import undo document; exported
files and ordinary settings remain intact. Importing profiles does not apply
them to the active displays.
A file that is readable JSON is not necessarily a valid Tomari backup.
If the current configuration changes after the preview, preview again before
replacing it so the confirmation refers to the state actually being replaced.

Tomari must also be able to read and validate the complete current configuration
before replacing it. Repair or remove quarantined keyboard items first; an
unreadable database belongs to the dedicated startup recovery flow described
above. Import does not silently discard damaged rows to manufacture a partial
restore point.

The database replacement and its pre-import restore point are saved together.
A database write failure leaves the existing configuration and any earlier
restore point intact. After a successful replacement, Tomari updates the live
keyboard rules, shortcuts, general preferences, and saved-position context.
It does not move open windows to imported positions automatically. Normal
permission and operating-system constraints still apply: a saved configuration
can produce a visible apply warning when macOS cannot activate part of it. That
warning means the import was saved, and the previous configuration remains
available to restore; it is not reported as a clean apply.

**Restore Previous Settings…** previews the configuration saved immediately
before the latest successful import. It remains available after quitting and
reopening Tomari. Confirming it replaces the current configuration, **including
edits made after the import**. This restores one configuration snapshot rather than
undoing individual edits. A subsequent successful import replaces that restore
point; a failed import does not. A successful restore consumes it. If macOS
cannot fully apply the restored settings, the remaining apply warning still
needs attention even though the database restoration succeeded.

Saved-position edit history is cleared when a whole configuration is replaced
so an old Remember/Forget undo cannot write a pre-import position back into the
new configuration. Shortcuts and Settings edits are serialized with the import;
an earlier settings autosave cannot silently overwrite the newly imported
preferences afterward.

The [runtime validation guide](runtime-validation.md) includes import, recovery,
and restart checks. Those physical-device checks are currently **Not run**;
automated validation does not establish that every macOS side effect succeeds.

## Localization

The Tomari window and tray menu are available in **Japanese and English**. By
default Tomari follows your system language; you can also pick a language
explicitly in the General section.


## Displays

The Displays page shows a proportional overview of connected screens, logical
and rendering resolutions, HiDPI availability, refresh rates, and native hardware
brightness where supported, plus session-only software dimming with a menu-bar
reset. Resolution/rate changes, relative arrangement, and
main-display selection use a 20-second preview with confirmation and automatic
rollback, even when the settings page is closed. A full desktop snapshot keeps
mode and position restoration together.
Named profiles save the complete resolution/rate/layout configuration for the
same connected screens, with fresh mode matching when recalled. Profiles are
included in settings backups; importing them does not change the live displays.

Display management is independent of Accessibility and Input Monitoring.
Mirrored configurations remain in macOS System Settings. Unsupported DDC/CI,
virtual-display, EDID, HDR-boost, and rotation controls are not offered.
See [Display management](displays.md) for the full behavior and recovery model.
