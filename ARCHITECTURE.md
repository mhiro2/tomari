# Architecture

How Tomari is structured: crate boundaries, runtime topology, and guidelines
for adding new features.

Tomari is a small utility that lives in the macOS menu bar. It currently
provides **keyboard customization** (modifier tap/hold, remapping, hyper key,
global shortcuts), **window management** (snapping to presets, moving across
displays, undo, drag-to-snap), and **sleep prevention** (keep awake, including
with the lid closed).

---

## 1. Design principles

- **Keep decision logic pure.** Tap/hold detection (`ModifierEngine`) and snap
  geometry (`geometry::compute_frame`) are pure implementations with zero OS
  dependencies, verified by unit tests. OS hooks such as CGEventTap and the
  Accessibility API stay thin: they feed events in and execute what comes out.
- **One action vocabulary.** Global shortcuts, modifier taps, the tray menu,
  UI buttons — every input path resolves to the same `AppAction` enum and goes
  through the same dispatcher (`actions::dispatch`). Adding an input path does
  not add action implementations.
- **Domain types are the JSON contract.** Types in `tomari-core` carry
  camelCase serde attributes and double as the DTOs exchanged with the
  frontend through Tauri commands. `src/lib/types.ts` mirrors them.
- **Features are added crate by crate.** A new tool is an independent
  `tomari-<feature>` crate (pure logic plus a macOS apply layer if needed) and
  a frontend tab.

## 2. Layers and crates

```text
┌─────────────────────────────────────────────┐
│ src/            React + TypeScript UI       │
│                 (one window: Keyboard /     │
│                  Window / Session /         │
│                  General tabs)              │
└──────────────────────┬──────────────────────┘
                       │ Tauri invoke (camelCase JSON)
┌──────────────────────▼──────────────────────┐
│ src-tauri/      Tauri v2 shell (tomari-app) │
│   commands / tray / shortcuts / actions     │
│   eventtap / drag_to_snap / drag_to_move /  │
│   keysend / window_ops  (macOS-specific)    │
└───────┬──────────────┬──────────────┬───────┘
        ▼              ▼              ▼
┌──────────────┐ ┌──────────────┐ ┌──────────────┐
│tomari-keyboard│ │tomari-window │ │ tomari-core  │
│ accelerator   │ │ geometry(pure)│ │ domain types │
│ engine(pure)  │ │ manager trait│ │ Database     │
│               │ │ macos(AX)    │ │ paths/clock  │
└───────┬──────┘ └───────┬──────┘ │ defaults     │
        └────────────────┴───────►└──────────────┘
```

| Crate                      | Role                                                                                                                                                                                                         |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `tomari-core`              | Domain types (`domain/`), `Error`, `AppPaths`, the SQLite `Database`, and `defaults` for first-run seeding. No OS dependencies                                                                               |
| `tomari-keyboard`          | `accelerator` (validation/normalization of shortcut strings) and `ModifierEngine` (tap/hold detection). All pure                                                                                             |
| `tomari-window`            | `geometry` (pure preset → frame computation), the `WindowManager` / `WindowHandle` traits plus `MockWindowManager` for tests, and `macos` (the Accessibility API implementation, `cfg(target_os = "macos")`) |
| `src-tauri` (`tomari-app`) | The menu-bar-resident Tauri v2 app. Tray, Tauri commands, global shortcuts, CGEventTap, action dispatch                                                                                                      |
| `src/`                     | React 19 + TypeScript window UI (pnpm workspace, Vite build)                                                                                                                                                 |

Dependencies point one way: `src-tauri` → `tomari-keyboard` / `tomari-window`
→ `tomari-core`. `tomari-core` and `tomari-keyboard` never touch OS APIs.
`tomari-window`'s macOS dependency is isolated in its `macos` module; on other
platforms `MockWindowManager` is plugged in instead (`make_window_manager` in
`main.rs`).

## 3. Domain model (`tomari-core::domain`)

- **`AppAction`** — the unified action vocabulary: `TogglePanel` /
  `SnapWindow(WindowPreset)` / `SnapWindowExact(WindowPreset)` (the exact
  variant applies the preset without the half→third→two-thirds cycle, so the
  URL scheme is idempotent) / `MoveWindowToDisplay` / `UndoWindow` /
  `SwitchIme(ImeMode)` / `SendKeystroke` / `ToggleKeepAwake` / `NoOp`.
  Round-trips to the frontend as-is via
  `#[serde(tag = "type", content = "value")]`.
- **`Hotkey`** — an accelerator string plus an `AppAction`.
- **`ModifierRule`** — for a modifier key (`ModifierKey` × `KeySide`):
  `remap_to` (the role it plays while held), `hyper` (hold acts as ⌃⌥⇧⌘), and
  `tap` (the action fired on a solo tap).
- **`WindowPreset`** (15 variants) / **`DisplayDirection`** / **`Rect`** —
  window-management value types. Coordinates are points with a top-left origin,
  matching both CGDisplay and the AX API.
- **`AppSettings`** — feature master switches, drag-to-snap configuration, the
  left/right ⌘ IME-toggle switch (`command_ime_switch_enabled`), UI language
  (`Language`: system / en / ja), etc. Persisted as a single JSON row. (The app
  is dark-only, so there is no theme setting; the tap/hold threshold is a fixed
  engine constant, not a preference.)

## 4. Input event flow (keyboard)

The heart of the keyboard feature is the persistent **CGEventTap** in
`src-tauri/src/eventtap.rs` (requires the Input Monitoring permission). A
dedicated thread owns the tap and runs a `CFRunLoop`; the callback observes
`flagsChanged` / `keyDown` / `keyUp`:

```text
CGEventTap (dedicated thread)
  ├─ modifier down/up ─► ModifierEngine.process()
  │     ├─ remap: rewrite the event's flags/keycode in place; while a remapped
  │     │   key is held, its target modifier is also stamped onto the keystrokes
  │     │   typed through it, so Control→Command + C registers as Cmd+C
  │     ├─ hyper: stamp ⌃⌥⇧⌘ onto keystrokes typed while held
  │     └─ solo tap completed ─► AppAction ─► actions::dispatch
  ├─ keyDown ─► Caps Lock (arriving as F18, see below) ─► drive as the Caps
  │     modifier, drop the F18 event; tap fires its action, held stamps its
  │     target. Other keyDowns pass through.
  └─ events Tomari itself synthesized (SYNTHETIC_MARKER) are ignored
```

Caps Lock is the exception. macOS delivers it as a _lock_ — one toggle event per
press, no key-up, and the AlphaShift lock applied below the tap — so the tap
alone can neither time a hold nor stop it locking. `src-tauri/src/capsmap.rs`
therefore remaps the Caps Lock HID usage to **F18** (an unused ordinary key) via
the OS `UserKeyMapping` facility (`hidutil`, Apple TN2450). The remap happens
before the lock is interpreted, so Caps never locks; F18 emits real key-down/up
that the tap drives as the Caps Lock modifier. `eventtap::restart` reconciles the
remap with whether an enabled rule manages Caps Lock, and quit takes it down.

- All decisions live in the pure engine; the tap only handles input and
  output. Timestamps are unified on `AppState::now_ms()` (an `Instant`
  origin), so the tap and the dispatch path produce comparable times.
- Key events synthesized by `keysend.rs` (`SwitchIme` posts the JIS 英数 0x66 /
  かな 0x68 keys; `SendKeystroke` resolves an accelerator to a keycode) are
  stamped with a marker in `EVENT_SOURCE_USER_DATA` so Tomari's own tap never
  enters a feedback loop.
- (Re)starting the tap is centralized in `eventtap::restart`, called when the
  feature is toggled or the permission is granted.

Global shortcuts are a separate channel registered with Tauri's
`global-shortcut` plugin (`shortcuts::register_all`). On fire, the handler
looks the shortcut up in `AppState::shortcuts` (`Shortcut → AppAction`) and
dispatches. Registration failures (invalid or conflicting accelerators) are
returned as errors so the UI can surface them.

## 5. Window management

Three layers:

1. **Pure geometry** (`tomari-window::geometry`) — `compute_frame` (preset →
   frame), `frames_match` (±2pt comparison tolerating windows that clamp to
   minimum sizes), `next_in_cycle` (the 1/2 → 1/3 → 2/3 cycle), `remap_frame`
   (proportional mapping across displays), and `edge_snap_preset` /
   `screen_at_cursor` (drag-to-snap: which preset a cursor at a screen
   border selects, on which display).
2. **Platform abstraction** (`manager`) — `WindowManager` (permission check,
   focused-window resolution, work-area enumeration) and `WindowHandle`
   (`frame` / `set_frame` / `stable_hash`). A handle can re-target the same
   window even after focus has moved elsewhere — it is the unit the undo
   history stores. The macOS implementation is `AxWindowManager` (direct
   bindings to the stable HIServices C functions).
3. **Orchestration** (`src-tauri/src/window_ops.rs`) — every input path goes
   through here. It honors the master switch and pushes "handle + previous
   frame" onto the undo history (`AppState::window_history`, capped at 50)
   only when something actually moved (decided via `frames_match`). Snaps
   remember a `LastSnap` (requested preset, applied preset, window identity
   hash, post-move frame) and advance the cycle only when the same preset is
   pressed again on the same, unmoved window.

**Drag-to-snap** (`drag_to_snap.rs`) is a second, listen-only CGEventTap, opt-in
and modifier-free: on mouse-down the window under the cursor is hit-tested; on
the first drag that actually moves its frame the drag arms. Edge detection needs
each display's full frame and work area, which only the main thread can read
(`WindowManager::screens_cg`) — so that geometry is **cached** in `AppState`
(primed at startup and refreshed whenever the displays change, via the
`NSApplicationDidChangeScreenParametersNotification` observer in `displays.rs`)
and the tap thread reads the cache, never blocking on a main-thread round-trip.
Armed drags then resolve the target purely from the cursor (`screen_at_cursor` +
`edge_snap_preset`), and only a change of target (preset _and_ display) touches
the preview. The preview is a translucent, click-through `NSPanel` in
`overlay.rs` — created lazily and held in a main-thread `thread_local!`, since
AppKit windows are not `Send` — driven from the tap thread through
`overlay::show` / `hide`, which hop to the main thread. On release the window
snaps to the previewed zone and the move is recorded for undo. A lost mouse-up
(tap disabled by the system) drops the drag and tears down its preview.

**Drag-to-move & resize** (`drag_to_move.rs`) is a third CGEventTap, opt-in and
modifier-gated. Unlike drag-to-snap it does not watch the OS move a window — it
_drives_ the window itself, so it is an **active** tap (`CGEventTapOptions::Default`):
on mouse-down it reads the held modifiers (`gesture_for_flags`: `⌃⌥` → move,
`⌃⌥⌘` → resize, Shift up), hit-tests the window under the cursor, and samples its
frame plus the cursor as the anchor. Each later drag applies a pure delta
(`geometry::drag_move_frame` / `drag_resize_frame`, the resize anchored at the
top-left and floored at `MIN_DRAG_SIZE`) straight through `DragWindow::set_origin`
/ `set_size`. While a gesture is in flight the mouse events are **consumed**
(`CallbackResult::Drop`) so the app underneath never sees the drag — which also
means the held Control cannot leak through as a secondary-click. A plain drag
with none of the gesture modifiers passes through untouched, and drag-to-snap
skips arming whenever a gesture chord is held so the two never fight.

## 6. Persistence (`tomari-core::db`)

- SQLite (rusqlite, bundled). `Database` wraps a single connection in a
  `Mutex`, with WAL and `foreign_keys = ON`.
- Migrations are `PRAGMA user_version` plus a single constant SQL schema
  (`SCHEMA`). The schema and the version bump run in one transaction, so a
  failure mid-setup rolls back cleanly (covered by tests). Post-release,
  schema changes add an incremental step rather than editing `SCHEMA`.
- Tables: `hotkeys` / `modifier_rules` / `settings` (a single `id = 1` row
  holding the `AppSettings` JSON). Domain values are stored as JSON strings in
  their columns, keeping the schema resilient to domain-type evolution.
- First-run seeding keys off the _absence of the settings row_
  (`build_state` in `main.rs`). Keying off empty tables would resurrect
  defaults whenever a user deliberately clears everything. Defaults live in
  `defaults.rs` (Caps Lock → Control — the one seeded modifier rule — plus the
  snap hotkeys). The left/right ⌘ IME toggle is _not_ a stored rule: it is
  assembled on demand from `command_ime_rules` when `command_ime_switch_enabled`
  is on.
- Storage location comes from `AppPaths` (`directories::ProjectDirs`,
  `tomari.sqlite`).
- Every config mutation — each interactive save/delete — holds
  `AppState::config_mutation`, so they serialize and the in-memory engines
  never disagree with disk.

## 7. Tauri shell and the frontend boundary

- `main.rs` is the assembly point: open and seed the DB → build `AppState`
  (DB, both engines, the `WindowManager`, the settings cache, the shortcut
  map, the undo history) → wire the plugins (single-instance / deep-link /
  autostart / updater / global-shortcut) and the tray → start the event tap
  and the drag-to-snap tap. `single-instance` is registered first: a second
  launch would create a duplicate event tap that double-fires every remap, so it
  hands off to the running instance (surfacing its panel) and exits.
  `deep-link` is registered right after it, as the plugin requires.
- The activation policy is **Accessory** (no Dock icon). A single window
  (`main`, 440×640, decorated, opaque, not always on top) is declared in
  `tauri.conf.json`; it carries the Keyboard / Window / Session / General tabs. Closing
  it is reinterpreted as hide (so reopening is instant and keeps state), and as
  a normal macOS window it stays open on focus loss. Minimize/zoom are disabled
  (`minimizable`/`maximizable: false`) so only the red close button is active.
  The global shortcut / modifier-tap / `tomari://v1/toggle-panel` toggle hides
  the window only when it is the active (visible and focused) window and
  otherwise raises it.
- **Permission polling**: Accessibility / Input Monitoring change in System
  Settings, outside the app, so a 2-second thread runs only the cheap status
  checks and rebuilds the tray menu on the main thread only on a change. When
  Input Monitoring is newly granted, the dead taps are restarted (a tap
  created without the permission is null and never revives on its own).
- **Tray** (`tray.rs`): setup items for missing permissions (at the very
  top), window snaps, Settings, Check for Updates (both open the single
  window; Check for Updates also emits `tomari:check-update`, which the UI
  handles by switching to the General tab and running the check). Rebuilt as
  permission state changes. Labels are localized (English / Japanese) from
  the language setting; `System` resolves via `NSLocale` and a language
  change rebuilds the menu.
- **Commands** (`commands.rs`): a thin CRUD + execution bridge invoked from
  the frontend. Save commands reflect changes into live state alongside
  persistence — saving a modifier rule calls the engine's `set_rules`, saving
  a hotkey calls `shortcuts::register_all`, and saving settings applies side
  effects only for the toggles that actually changed (so flipping an unrelated
  preference never tears down the event tap and briefly drops key monitoring) —
  flipping the ⌘ IME switch reassembles the engine's rules via
  `reload_engine_rules`. Commands reject with a `CmdError`
  (`{ code, message }`, `src-tauri/src/error.rs`): the frontend localizes the
  frequent `code`s (missing permission, no focused window, shortcut conflict)
  and falls back to the English `message` for the rest.
- **Frontend** (`src/`): `main.tsx` mounts a single `App` with four tabs —
  `KeyboardView` / `WindowView` / `SessionView` / `GeneralView`. `lib/api.ts` provides typed invoke wrappers whose argument
  keys must match the Rust command parameter names; `lib/types.ts` mirrors
  the domain types. `lib/i18n.tsx` holds the typed English/Japanese message
  dictionaries and the `useT` hook; backend commands return ids (e.g.
  `WindowPreset`) and the frontend renders the localized label. Shortcuts are
  recorded by `components/ShortcutRecorder.tsx`, which suspends the
  registered global shortcuts (`set_hotkeys_suspended`) while capturing a
  chord.
- **Updater**: `tauri-plugin-updater`. The `Update` found by
  `check_for_update` is held in `PendingUpdate` until `install_update`
  consumes it and relaunches. The endpoint is `latest.json` on GitHub
  Releases.
- **External control / URL scheme** (`tomari-core::external`,
  `dispatch_deep_link` in `main.rs`): launchers like Raycast/Alfred drive
  Tomari through `tomari://v1/...`. `tauri-plugin-deep-link` delivers URLs; the
  cold-start URL (`get_current`) and warm-start URLs (`on_open_url`) funnel
  through one handler — never argv. `parse_deep_link` validates strictly
  (versioned `v1`; no query/fragment/userinfo/port; unknown verbs or extra
  args rejected) into `ExternalAction`, a deliberately small allowlist — snap /
  move-display / undo / toggle-panel — that is the security boundary between an
  arbitrary caller and the open-ended `AppAction`:
  `ExternalAction → AppAction → dispatch`. Snap maps to `SnapWindowExact` so a
  repeated URL is idempotent. Window placement (snap / move-display / undo) is
  gated behind `external_window_actions_enabled` (default off, so external
  control is opt-in); `toggle-panel` is exempt — it only shows/hides Tomari's
  own panel and is the recovery route for a hidden menu bar. Fire-and-forget,
  so a malformed URL or the disabled switch is logged and dropped rather than
  surfaced.

## 8. Keep awake (`src-tauri/src/keepawake.rs`)

Sleep prevention for long-running background work — e.g. an AI agent that must
keep running after the laptop lid is shut. Two layers, because macOS treats them
differently:

- An **IOKit power assertion** (`PreventUserIdleSystemSleep`) blocks idle system
  sleep. It needs no permission and is released cleanly — but macOS deliberately
  ignores it once the lid closes (a thermal safety choice), so on its own it
  only covers the lid-open case.
- **`pmset disablesleep`** sets the kernel `SleepDisabled` flag, which also
  vetoes lid-close (clamshell) sleep. It needs administrator rights — engaged
  through the standard auth dialog (`osascript … with administrator privileges`,
  run on a worker thread so the dialog never blocks the caller) — and persists
  until cleared.

The lid-close veto is a **required** part of keep-awake, not an optional add-on,
and both directions go through it on the worker thread — which (not the toggle)
commits the `active` flag. Turning on takes the idle assertion immediately and
shows on; if the veto then cannot be engaged (auth declined, or the sleep state
is unreadable) the whole switch rolls back off. Turning off is deferred to the
worker: clearing the override needs an admin dialog that can be declined, and
sleep is still prevented until it succeeds, so a declined clear keeps keep-awake
on. A `generation` counter, bumped on every toggle, lets a slow worker detect
that a newer toggle superseded it while its auth dialog was up, so a stale cancel
never clobbers a switch the user has since re-toggled (the pure
`reconcile_writeback` decides supersede / on / off and is unit-tested).

Keep-awake is **runtime state** in `AppState` (`Mutex<KeepAwake>`), never
persisted: it always starts off at launch. A toggle reaches it from the tray (a
`CheckMenuItem`), the panel (`get_keep_awake` / `set_keep_awake` commands), and
`AppAction::ToggleKeepAwake` (hotkeys / taps). Every change emits
`tomari:keep-awake-changed` and rebuilds the tray, so the panel toggle and the
tray checkmark stay in sync regardless of which surface initiated it.

Because `disablesleep` survives a crash, a marker file under the data directory
records that _we_ engaged it. `reconcile_on_launch` (from `setup`) clears a
leftover override — only one we set, never a user's own `disablesleep` — and
`cleanup_blocking` (from `RunEvent::ExitRequested`, covering tray Quit, updater
relaunch and logout alike) releases everything before the process exits. The
pure `reconcile_decision` is unit-tested; the IOKit / `pmset` layer stays thin.

## 9. Permission model

| Permission       | Required by                                        | Acquisition                                                                                                                            |
| ---------------- | -------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| Accessibility    | Moving windows (AX), key synthesis (`keysend`)     | `AXIsProcessTrustedWithOptions` (with prompt)                                                                                          |
| Input Monitoring | The keyboard tap and the drag-to-snap tap          | `CGRequestListenEventAccess`. Attempting to create a tap without it adds Tomari to the Input Monitoring list so the user can enable it |
| Administrator    | Keep-awake's lid-close veto (`pmset disablesleep`) | macOS auth dialog via `osascript … with administrator privileges`; required to turn Keep Awake on — the lid-close veto is part of the switch, so declining cancels it |

Global shortcuts need neither permission. The pure engines are testable
without permissions too (unit tests).

## 10. Testing

- **Rust**: pure logic (the engine, geometry, accelerators) is
  covered by in-module unit tests. The DB opens in memory; tests cover the
  migration creating the full schema (every table and column). Window
  operations are tested without the OS via `MockWindowManager`.
- **Frontend**: Vitest + Testing Library (jsdom). `vitest.setup.ts` mocks the
  Tauri API.
- **Toolchain**: clippy (`-D warnings`) / oxlint (type-aware) for linting,
  rustfmt / oxfmt for formatting, tsgo for type checking, cargo-deny for
  dependency auditing. `make check` runs the whole local suite.
- **CI** (GitHub Actions): four jobs — frontend (ubuntu), Rust tests (macos),
  cargo-deny, and the macOS bundle build.

## 11. Adding a feature

1. If it needs domain types and persistence, add the types to `tomari-core`,
   bump `SCHEMA_VERSION`, and add one migration step (keep existing rows
   alive with additive defaults).
2. Put decision/computation logic in a new `tomari-<feature>` crate (or an
   existing one) as **pure functions / pure state machines**, with unit
   tests. Isolate OS dependencies behind a trait or a `cfg(target_os)`
   module.
3. If users trigger it, add one variant to `AppAction` and one branch to
   `actions::dispatch`. That alone makes it reachable from hotkeys, taps,
   the tray, and the UI.
4. UI work is a tab under `src/views/` plus additions to `lib/api.ts` /
   `lib/types.ts`. Add a thin Tauri command in `commands.rs` and register it
   in the handler list in `main.rs`.
5. In save commands, remember to sync persistence with live state (engines,
   shortcut registration, taps). Restart a tap only when the change truly
   requires it.
