# Feature reference

A detailed look at everything Tomari can do. For a quick overview, see the
[README](../README.md).

## Keyboard

- **Modifier remapping** — remap Caps Lock / Control / Option / Command /
  Shift / fn to another modifier (for example, _Caps Lock → Control_).
- **Tap vs. hold** — tap a modifier on its own, quickly, to fire a dedicated
  action; keep holding it and it behaves as the normal modifier.
- **IME switching with left/right ⌘** — tap the left ⌘ to switch to English
  (Eisū), tap the right ⌘ to switch to Japanese (Kana).
- **Tap actions** — a modifier tap can show/hide the panel, snap a window,
  launch an app (Quick Peek), switch the IME, or send an arbitrary key.
- **Hyper key** — while a modifier is held, fire ⌃⌥⇧⌘ together. This gives you
  a dedicated hotkey range that rarely collides with existing shortcuts.
- **Global shortcuts** — bind any action to an accelerator (for example,
  `Ctrl+Alt+Left`). In the settings UI you record a shortcut by clicking the
  field and simply pressing the keys you want.

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
handles as the Caps Lock modifier (tap for the bound action, hold to act as the
remapped modifier).

This needs no extra setup: the remap is applied automatically while a Caps Lock
rule is enabled and removed when Tomari quits or the rule is turned off. A few
consequences to be aware of:

- It replaces any custom `hidutil` key mapping you may have set yourself.
- While a Caps Lock rule is active, a physical **F18** key (uncommon) is treated
  as Caps Lock too — the remap makes them indistinguishable.
- If Tomari is force-quit (rather than quit normally) the remap can persist until
  the next launch, which removes it again; you can also clear it yourself with
  `hidutil property --set '{"UserKeyMapping":[]}'`.

## Window management

- Snap the focused window to one of 15 presets: left/right halves, quarters,
  thirds, maximize, centering, and more.
- Trigger snapping from the menu bar menu, a global shortcut, or the grid in
  the UI.
- **Drag-to-snap (optional)** — drag a window to a screen edge or corner to
  show a preview, then release to snap to a half, a corner, or full screen
  depending on where you let go.
- **Multi-display** — move the focused window to the neighboring display,
  placed proportionally.
- **Cycling** — pressing the same snap hotkey repeatedly cycles through related
  sizes (for example 1/2 → 1/3 → 2/3).
- **Undo** — restore the window's previous position.

## Keep Awake

Keep long-running jobs from AI agents (Codex, Claude Code, and the like) from
being interrupted — **even when the display is closed**. You can toggle it
manually from the menu bar tray, the toggle in the settings tab, or a global
shortcut (the "Toggle keep awake" action). Automatic process detection is
planned for the future.

How it works, in two layers:

1. An IOKit power assertion (`PreventUserIdleSystemSleep`) prevents idle sleep.
   This needs no special permission.
2. macOS ignores that assertion once the display is closed, so while Keep Awake
   is on Tomari also runs `pmset disablesleep` to keep working with the lid
   closed. **This requires your administrator password.**

The state lasts **only for the current session**: Tomari always starts with
Keep Awake off. Even if the app crashes while it is on, a consistency check at
launch and cleanup at exit reliably clear `disablesleep`, so you are never left
in a "won't sleep" state.

Running with the lid closed increases battery drain and heat, so keeping the
machine plugged in is recommended.

## Backup (Import / Export)

- Export **all** of your configuration — app settings, hotkeys, and modifier
  rules — to a **single JSON file**, and import it back. Use the "Backup"
  section of the settings tab.
- The output is human-readable, **pretty-printed JSON** (stably sorted by id,
  with a trailing newline; the app version is not included), so it diffs
  cleanly.
- Importing **fully replaces** your current configuration. Before applying, the
  file is **strictly validated**: if there is even one duplicate id or invalid
  accelerator, **nothing is changed** and the problems are listed. (Numeric
  settings are the one exception — they are clamped into range with a warning.)
- Just before replacing, the current database is **automatically backed up** to
  `backups/pre-import-<timestamp>.sqlite` in the data directory (a complete
  copy via `VACUUM INTO`, so even unreadable rows are preserved). To restore,
  quit Tomari and move the file back to `tomari.sqlite`.
- **Launch at login** is treated as machine-specific: it is recorded in the
  file but **not applied on import**, so importing never changes the startup
  items on a different machine.
- The file format is versioned with `formatVersion` (currently `1`); unknown,
  newer versions are explicitly rejected.

## Localization

The settings panel and tray menu are available in **Japanese and English**.
By default Tomari follows your system language; you can also pick a language
explicitly in settings.
