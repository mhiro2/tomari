# Tomari

Tomari gathers a handful of small macOS utilities under **a single menu bar
icon**. Today it ships **keyboard customization** and **window management**,
and it is built so that new tools can be added on the same foundation over
time.

## Features

### Keyboard

- **Modifier remapping** — Caps Lock / Control / Option / Command / Shift / fn
  (for example, _Caps Lock → Control_).
- **Tap vs. hold** — tap a modifier on its own to fire a dedicated action; hold
  it and it stays a normal modifier.
- **IME switching with left/right ⌘** — left ⌘ for English, right ⌘ for
  Japanese (Kana).
- **Hyper key** — fire ⌃⌥⇧⌘ together for a collision-free hotkey range.
- **Global shortcuts** — bind any action to an accelerator; record it just by
  pressing the keys.

### Window management

- Snap the focused window to one of 15 presets — halves, quarters, thirds,
  maximize, center, and more.
- Trigger from the menu bar, a global shortcut, or the grid in the UI.
- Drag a window to a screen edge or corner to snap on release (optional).
- Multi-display moves, size cycling, and undo.

### Keep Awake

- Keep long-running jobs alive **even with the lid closed**. Toggle it from the
  tray, settings, or a global shortcut.

### Backup

- Export and import **all** of your settings as a single, human-readable JSON
  file, with strict validation and an automatic backup on import.

### And more

- **Localized UI** in English and Japanese (follows your system language by
  default).

See the [feature reference](docs/features.md) for the full details.

## Installation

**Requirements:** macOS 26 or later on Apple Silicon.

1. Download the latest `Tomari_*.dmg` from the
   [Releases](https://github.com/mhiro2/tomari/releases) page.
2. Open the DMG and drag **Tomari** into your Applications folder.
3. Tomari is a canary release and is not yet notarized, so Gatekeeper blocks it
   on first launch. Right-click the app and choose **Open**, then confirm — or
   run `xattr -dr com.apple.quarantine /Applications/Tomari.app`.
4. Launch Tomari and grant the permissions it needs. See the
   [permissions guide](docs/permissions.md).

> [!WARNING]
> Tomari is an early canary release — expect rough edges and breaking changes between versions.

## Documentation

- [`ARCHITECTURE.md`](./ARCHITECTURE.md) — workspace layout and runtime topology.
- [Feature reference](docs/features.md) — every feature in detail.
- [Permissions & privacy](docs/permissions.md) — Accessibility, Input Monitoring, and Keep Awake.
- [URL scheme](docs/url-scheme.md) — drive window actions from Raycast, Alfred, and other launchers.

## License

Tomari is released under the [MIT License](LICENSE).
