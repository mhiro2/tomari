# Feature reference

A detailed look at everything Tomari can do. For a quick overview, see the
[README](../README.md).

## Keyboard

The **Keyboard** settings separate the two kinds of configuration into
**Modifier Keys** and **Shortcuts** tabs. Modifier Keys presents each physical
key as a row with its tap action, held role, and enabled state; the left/right
Command IME switch is shown as a direct key-to-input-source mapping. Global
shortcut recording stays in the Shortcuts tab so it does not compete with the
modifier table.

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

### Notes on remapping

Remapping rewrites event flags and key codes at the event-tap level. Control,
Option, Command, Shift, and fn can be tracked per side for both press and
release, so they remap correctly as momentary modifiers (active only while
held).

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

Push the status icons you rarely look at off the edge of the screen, and bring
them back when you want them.

Turn it on in the **Menu Bar** section and Tomari adds two small items to your
menu bar: a **divider** (≡) and a **handle** (‹). Collapsing stretches the
divider so everything to its *left* slides off-screen; the handle stays put so
you can always bring them back.

**You choose what gets hidden.** Hold ⌘ and drag your menu bar icons so the ones
you want tucked away sit to the left of the divider. macOS lets an app move only
its own menu bar icons, so this part is yours to do — there is no way for Tomari
to sort them for you.

The **Menu Bar** settings separate **Items** from **Behavior**. Items shows a
menu-bar diagram and a live, best-effort inventory split into **Hidden now** and
**Always shown**. After moving an icon across the divider with ⌘-drag, choose
**Refresh Items** to reread the physical arrangement. Reading item names
requires Accessibility access; some applications expose only a generic owner
name, and the list may be incomplete when macOS does not publish an item
through Accessibility. Behavior contains the show/hide control and automatic
collapse timing.

- **Expand and collapse** — click the ‹ handle, use **Show Menu Bar Icons** in
  the tray menu, or bind the "Show/Hide Menu Bar Icons" action to a shortcut in
  the Keyboard section.
- **Collapse automatically** — off by default; 5, 15 or 30 seconds are
  available. A timed collapse fires on schedule whatever you are doing,
  including while one of the revealed menus is open.
- **Limits** — expanding may not reveal everything if the frontmost app has a
  long menu bar of its own, or if your Mac has a notch. That is the method, not
  a bug: Tomari is making room by moving its own item, not by taking over the
  menu bar.

If you ever lose the handle (⌘-dragging it to the *left* of the divider hides it
along with everything else), open Tomari's window with the global shortcut
(default ⌘⇧K) and use the switch in the Menu Bar section.

## Prevent Sleep (keep awake)

Keep long-running jobs from AI agents (Codex, Claude Code, and the like) from
being interrupted — **even when the display is closed**. You can toggle it
manually from the menu bar tray, the toggle in the Prevent Sleep section, or a global
shortcut (the "Toggle Prevent Sleep" action). Automatic process detection is
planned for the future.

How it works, in two layers that engage together:

1. An IOKit power assertion (`PreventUserIdleSystemSleep`) prevents idle sleep.
2. macOS ignores that assertion once the display is closed, so Tomari also runs
   `pmset disablesleep` to keep working with the lid closed. **This requires your
   administrator password.**

Both layers are part of one switch, and turning it **on or off** needs the
password: declining when enabling cancels Prevent Sleep entirely (no display-open
fallback), and declining when disabling leaves it on (sleep is still prevented
until the override is cleared).

The state lasts **only for the current session**: Tomari always starts with
Prevent Sleep off. Even if the app crashes while it is on, a consistency check at
launch and cleanup at exit reliably clear `disablesleep`, so you are never left
in a "won't sleep" state.

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
a master switch. Turning a feature off leaves its settings visible so its scope
is still understandable, but disables the controls until the feature is turned
back on. Windows, Keyboard, and Menu Bar use the focused tabs described above;
ordinary options use divided rows, while cards are reserved for objects such as
saved window positions.

On the very first launch the settings window opens automatically (the main
features need permissions you have not granted yet); after that Tomari starts
silently in the menu bar. The same happens on the rare launch where Tomari had
to reset an unreadable settings database — your settings are back at their
defaults, so the window shows you what state you are in.

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

## Localization

The Tomari window and tray menu are available in **Japanese and English**. By
default Tomari follows your system language; you can also pick a language
explicitly in the General section.
