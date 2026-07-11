# Feature reference

A detailed look at everything Tomari can do. For a quick overview, see the
[README](../README.md).

## Keyboard

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
  entry, leaving your other mappings in place.
- While a Caps Lock rule is active, a physical **F18** key (uncommon) is treated
  as Caps Lock too — the remap makes them indistinguishable.
- If Tomari is force-quit (rather than quit normally) the remap can persist until
  the next launch, which removes it again; you can also clear just Tomari's entry
  yourself by running `hidutil property --get UserKeyMapping`, removing the
  Caps Lock → F18 entry, and setting the rest back.

## Window management

- Snap the focused window to one of 15 presets: left/right halves, quarters,
  thirds, maximize, centering, and more.
- Trigger snapping from the menu bar menu, a global shortcut, or the grid in
  the UI. Triggering it from the UI always targets the window you were using
  before you opened Tomari, never the settings window itself.
- **Default snap shortcuts** — `⌃⌥←` / `⌃⌥→` / `⌃⌥↑` for left half, right half,
  and maximize. `⌃⌥` (Control + Option) is the Mac-native modifier pair used by
  most window managers and does not collide with macOS's own `⌃`+arrow (Spaces
  and Mission Control). All shortcuts are rebindable in the Keyboard tab.
- **Drag-to-snap (optional)** — drag a window to a screen edge or corner to
  show a preview, then release to snap to a half, a corner, or full screen
  depending on where you let go.
- **Drag-to-move & resize (optional)** — hold `⌃⌥` and drag anywhere inside a
  window to move it, or `⌃⌥⌘` to resize it from the bottom-right (the top-left
  corner stays anchored). It acts on the window under the pointer with no need
  to click it first, and while a gesture is held the drag is consumed so the app
  underneath never sees it.
- **Multi-display** — move the focused window to the neighboring display,
  placed proportionally.
- **Cycling** — pressing the same snap hotkey repeatedly cycles through related
  sizes (for example 1/2 → 1/3 → 2/3).
- **Undo** — restore the window's previous position.

## Prevent Sleep (keep awake)

Keep long-running jobs from AI agents (Codex, Claude Code, and the like) from
being interrupted — **even when the display is closed**. You can toggle it
manually from the menu bar tray, the toggle in the Session tab, or a global
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

Tomari runs as a menu bar app: clicking its menu bar icon opens a menu with
quick actions plus a Settings entry that opens the window (Keyboard, Window,
Session, and General tabs). You can hide the icon with **Show in menu bar** in
the General tab if you prefer a fully background app. Because hiding it removes
the app's only visible affordance (Tomari has no Dock icon), turning it off asks
you to confirm first and spells out how to reopen the window.

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
explicitly in the General tab.
