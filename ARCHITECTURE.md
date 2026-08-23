# Architecture

How Tomari is structured: crate boundaries, runtime topology, and guidelines
for adding new features.

Tomari is a small utility that lives in the macOS menu bar. It currently
provides **keyboard customization** (modifier tap/hold, remapping, hyper key,
global shortcuts), **window management** (per-application remembered homes,
reversible restore, quick tiling, drag-to-snap), **menu bar tidying** (hiding
status items behind a divider), and **sleep prevention** (keep awake, including
with the lid closed).

---

## 1. Design principles

- **Keep decision logic pure.** Tap/hold detection (`ModifierEngine`) and window
  geometry (`compute_frame`, normalization, recall) are pure implementations
  with zero OS dependencies, verified by unit tests. OS hooks such as CGEventTap
  and the Accessibility API stay thin: they feed events in and execute what
  comes out.
- **One action vocabulary.** Global shortcuts, modifier taps, the tray menu,
  UI buttons — every input path resolves to the same `AppAction` enum and goes
  through the same dispatcher (`actions::dispatch`). Adding an input path does
  not add action implementations.
- **Domain types are the JSON contract.** Types in `tomari-core` carry
  camelCase serde attributes and double as the DTOs exchanged with the
  frontend through Tauri commands. `src/lib/types.ts` mirrors them.
- **Features are added crate by crate.** A new tool is an independent
  `tomari-<feature>` crate (pure logic plus a macOS apply layer if needed) and
  a frontend section.

## 2. Layers and crates

```text
┌─────────────────────────────────────────────┐
│ src/            React + TypeScript UI       │
│                 (one window, five direct    │
│                  sidebar destinations:      │
│                  Windows / Keyboard /       │
│                  Menu Bar / Prevent Sleep / │
│                  General)                   │
└──────────────────────┬──────────────────────┘
                       │ Tauri invoke (camelCase JSON)
┌──────────────────────▼──────────────────────┐
│ src-tauri/      Tauri v2 shell (tomari-app) │
│   commands / tray / shortcuts / actions     │
│   eventtap / drag_to_snap / drag_to_move /  │
│   keysend / window_ops / menubar            │
│   (macOS-specific)                          │
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
  URL scheme is idempotent) / `MoveWindowToDisplay` /
  `RecallWindowPlacement` / `MoveWindowToDisplayAndRecall` / `UndoWindow` /
  `RedoWindow` / `SwitchIme(ImeMode)` / `SendKeystroke` /
  `ToggleKeepAwake` / `ToggleMenuBar` / `NoOp`.
  Round-trips to the frontend as-is via
  `#[serde(tag = "type", content = "value")]`.
- **`Hotkey`** — an accelerator string plus an `AppAction`.
- **`ModifierRule`** — for a modifier key (`ModifierKey` × `KeySide`):
  `remap_to` (the role it plays while held), `hyper` (hold acts as ⌃⌥⇧⌘), and
  `tap` (the action fired on a solo tap).
- **`WindowPreset`** (15 variants) / **`DisplayDirection`** / **`Rect`** —
  immediate window-management value types. Coordinates are points with a
  top-left origin, matching both CGDisplay and the AX API.
- **`WindowApplication`** / **`PlacementSlot`** / **`NormalizedRect`** /
  **`WindowPlacement`** — the privacy-safe remembered-home model. A bundle id
  is the durable app identity, each app has only Primary and Secondary slots,
  shown as Position A and Position B in the UI, and the frame is relative to
  one display's usable work area. Window titles are neither required nor
  persisted.
- **`AppSettings`** — feature master switches, drag-to-snap configuration, the
  left/right ⌘ IME-toggle switch (`command_ime_switch_enabled`), menu bar
  tidying (`menu_bar_tidy_enabled` plus its `menu_bar_auto_collapse_secs`), UI
  language (`Language`: system / en / ja), etc. Persisted as a single JSON row.
  (The app is dark-only, so there is no theme setting; the tap/hold threshold
  is a fixed engine constant, not a preference.)

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
Because setting the property rewrites the whole list, both directions read the
current `UserKeyMapping` first and add/remove only Tomari's Caps Lock → F18
entry, leaving any mappings the user set themselves intact. That read is what
the whole property is rebuilt from, so `parse_entries` is strict — an output
format it does not fully understand, an extra field, an unexpected number
format, a missing separator, a source mapped twice or a truncated read is an
error, never a partial list, since writing a partial list back would delete the
entries it failed to read. The one mapping that cannot merely coexist is one the
user put on the Caps Lock source itself (Caps → Escape, say): taking that slot
over has to replace it, and `UserKeyMapping`
records no provenance, so a live Caps Lock → F18 is equally plausibly the user's
own. Ownership is therefore explicit, in a `capsmap.claim` record in the data
directory. Nothing commits the record and the OS property together, so the
record separates intending to take the slot from having taken it: absent (Tomari
holds nothing — a live Caps Lock → F18 is the user's and is left alone),
`pending [usage]` (write-ahead, OS write unconfirmed — a live Caps Lock → F18 is
unattributable, so this state only ever gives the claim up), or `held [usage]`
(confirmed, and the optional usage is what was displaced for the release to
restore). Every transition fails closed: a record that cannot be written, or on
the way back cannot be read, aborts it rather than risk the user's mapping.
Taking over always rewrites the record, so a stale one left by a crash is
replaced rather than deleted; every release is gated on our entry still being
live, so a change made outside Tomari in the meantime wins and the claim is
dropped unused. `reconcile` runs the release direction whenever Caps Lock should
not be managed, which is what stops a stale claim outliving the mapping it
described, and serializes the whole sequence — live read, record, OS write, and
the F18-proxy flag the tap reads — on one mutex, since settings commands, wake,
the permission poll and quit can all reach it concurrently. A writer outside
Tomari cannot be locked out and the property offers no atomic swap, so each
write is bracketed by the checks that are possible: the live list has to still
be the one the plan was built from (otherwise we do not write, and the next
reconcile re-plans), and our own entry has to afterwards be what we wrote. A
write that lands in between is still lost — the brackets narrow the window
rather than closing it.

- All decisions live in the pure engine; the tap only handles input and
  output. Timestamps are unified on `AppState::now_ms()` (an `Instant`
  origin), so the tap and the dispatch path produce comparable times.
- Key events synthesized by `keysend.rs` (`SwitchIme` posts the JIS 英数 0x66 /
  かな 0x68 keys; `SendKeystroke` resolves an accelerator to a keycode) are
  stamped with a marker in `EVENT_SOURCE_USER_DATA` so Tomari's own tap never
  enters a feedback loop.
- (Re)starting the tap is centralized in `eventtap::restart`, called when the
  feature is toggled or the permission is granted. The scaffolding underneath
  all three CGEventTaps (this one, drag-to-snap, drag-to-move) — spawn a
  dedicated thread, create the tap, attach its run-loop source, and hand the
  running `CFRunLoop` back so `Drop` can stop and join it, plus re-arming after
  `TapDisabledByTimeout`/`TapDisabledByUserInput` — is itself centralized in
  `tap::spawn`/`tap::reenable` (`src-tauri/src/tap.rs`); only the tap name,
  `CGEventTapOptions`, watched event types, and the callback differ per tap.
- Neither half of that lifecycle waits forever: starting waits at most
  `START_DEADLINE` for the run loop to report in, stopping at most
  `STOP_DEADLINE` for the thread to return. The callers are the settings save,
  the wake handler and quit, and a callback stuck in an OS call it cannot cancel
  must not turn any of those into a hang. Past a deadline the thread is
  *detached*, so its `CGEventTap` can still be live when the next one starts —
  which is why every tap carries a liveness flag that `RunningTap::drop` clears
  before stopping the run loop, and the wrapper around the callback returns
  early once it is clear. A detached tap still exists but handles nothing
  further. An event it had already started on runs to completion and its verdict
  stands — its side effects are committed by then, so discarding only the
  verdict would hand the app an event whose consequences had happened anyway.
  The startup hand-over and the caller's deadline go through one mutex, so a run
  loop that starts at the very moment the caller gives up is told to stop rather
  than left running with nobody holding it.

Global shortcuts are a separate channel registered with Tauri's
`global-shortcut` plugin (`shortcuts::register_all`). On fire, the handler
looks the shortcut up in `AppState::shortcuts` (`Shortcut → AppAction`) and
dispatches. Registration failures (invalid or conflicting accelerators) are
returned as errors so the UI can surface them.

## 5. Window management

Three layers:

1. **Pure geometry** (`tomari-window::geometry`) — `compute_frame` (preset →
   frame), `normalize_frame` / `recall_frame` (safe display-relative homes),
   `frames_match` (±2pt comparison tolerating windows that clamp to minimum
   sizes), `next_in_cycle` (the 1/2 → 1/3 → 2/3 cycle), `remap_frame`
   (proportional mapping across displays), and `edge_snap_preset` /
   `screen_at_cursor` (drag-to-snap: which preset a cursor at a screen border
   selects, on which display).
2. **Platform abstraction** (`manager`) — `WindowManager` (permission check,
   focused-window resolution, work-area enumeration) and `WindowHandle`
   (`frame` / `set_frame` / `stable_hash`). A handle can re-target the same
   window even after focus has moved elsewhere — it is the unit the undo
   history stores. The macOS implementation is `AxWindowManager` (direct
   bindings to the stable HIServices C functions). Focused-window resolution
   normally reads the system-wide `AXFocusedApplication`/`AXFocusedWindow`, but
   if that application turns out to be Tomari itself (e.g. a click landed on
   the settings window), it falls back to the frontmost *other* app's focused
   window via the on-screen `CGWindowList`, so an operation triggered from
   Tomari's own UI never targets Tomari's own window. `FocusedWindow` also
   carries the owning application's bundle id and localized name; it does not
   read or expose the window title.
3. **Orchestration** (`src-tauri/src/window_ops.rs`) — every input path goes
   through here. It honors the master switch and records a `WindowChange`
   containing the handle plus both before and after frames only when something
   actually moved (decided via `frames_match`). Undo and redo share this one
   in-memory history, capped at 50, regardless of whether a change came from a
   remembered home, display move, preset, or drag. A new change clears redo.
   History availability is queried independently of focused-window context,
   and undo/redo return whether they applied a frame, found no entry, or only
   discarded handles for windows that have closed. Remembered-home data edits
   use a separate capped recovery stack because restoring persisted data is a
   different operation from restoring a live window frame.
   Snaps separately remember a `LastSnap` and advance the cycle only for the
   same preset on the same unmoved window.

`window_placements` persists at most two homes per bundle id. Capture clamps a
window inside its current usable work area and normalizes its frame to 0…1;
recall expands that value into the current display's work area, so display
reconnection or a different resolution does not reapply stale global pixels.
The ordinary recall action chooses Primary first and alternates to Secondary
when repeated on the same unmoved window. Move-and-recall selects the adjacent
display and applies Primary there as one history entry, falling back to
Secondary only when Primary is absent.

The settings panel receives a `WindowTarget` containing the app bundle id and
an opaque stable window identity with every placement context. Capture, forget,
recall, and move-and-recall send that identity back; orchestration resolves the
focused handle once, rejects a mismatch, and then operates on that exact handle.
Panel-show and focus events still refresh the context promptly, while this
backend check closes the focus-change race rather than relying on refresh timing.
Focused-context reads retry `kAXErrorCannotComplete` once before returning a
localized, actionable error; the retry remains before identity validation and
before any window mutation.

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
The mouse-down frame is retained as the history entry's `before` value, so Undo
returns to the true drag origin rather than the temporary screen-edge frame at
release.
`overlay` gives every issued `show`/`hide` a fresh generation and applies a
queued operation on the main thread only while its generation is still the
current one — last writer wins, with no assumption about delivery order — so a
stale `show` still queued when the tap is torn down can never resurrect a
preview after the teardown's `hide`, nor can a stale `hide` clear a newer one.

**Drag-to-move & resize** (`drag_to_move.rs`) is a third CGEventTap, opt-in and
modifier-gated. Unlike drag-to-snap it does not watch the OS move a window — it
_drives_ the window itself, so it is an **active** tap (`CGEventTapOptions::Default`),
whose callback holds up **all** input while it runs, Tomari's own or not. So the
callback calls into no other process, starts no thread, joins none and takes no
lock: it reads the held modifiers (`gesture_for_flags`: `⌃⌥` → move, `⌃⌥⌘` →
resize, Shift up) plus two atomics — `ENABLED`, mirrored out of the settings by
`restart_result`, and `ACCESSIBILITY`, mirrored from the permission poller —
then posts a `Command` down a channel and returns.

Everything that messages the target app happens on the single applier thread
started with the tap — the hit-test that finds the window, the frame read that
anchors the drag, and each delta applied after it
(`geometry::drag_move_frame` / `drag_resize_frame`, the resize anchored at the
top-left and floored at `MIN_DRAG_SIZE`) through `DragWindow::set_origin` /
`set_size`. One thread for the whole tap, not one per gesture, is what keeps two
gestures' Accessibility calls from overlapping: an ended gesture's last write
cannot still be landing while the next gesture reads its anchor, because both are
steps on the same thread. Every command carries its gesture's generation, and
the queue is drained before every call, so a gesture that began and ended while
an earlier call was in flight is discarded without a call of its own, and a slow
write is followed by the newest cursor rather than a backlog. The guarantee is
precisely that no call is *started* for a gesture already known to be over — a
release arriving after the drain, or while a call is already running, cannot
cancel it. What to do next is decided by a pure `next_step`, so that much is
settled by tests rather than by scheduling.

Because the hit-test is no longer synchronous, consuming the press commits
before it is known whether anything under the cursor is draggable — a chord
press over nothing draggable is swallowed rather than passed on, which is the
deliberate trade for never stalling input. Ownership of the press is tracked
separately from whether a gesture is still being driven, so a press that was
consumed still has its release consumed after a gesture is cut short by a tap
the system disabled mid-drag. The one case not covered is the tap going away
mid-press (the feature switched off while the button is held): the callback is
gone, so the release passes through. Deferring teardown until the user lets go
would block a settings save on a human. A plain drag with none of the gesture
modifiers passes through untouched, and drag-to-snap skips arming whenever a
gesture chord is held so the two never fight.

`DragToMoveState::drop` ends the current gesture, closes the channel and joins
the applier. It runs on the tap thread as it shuts down — off the input path,
and strictly before `RunningTap::drop`'s thread join returns — so no
Accessibility call from a torn-down tap can still be in flight once the next tap
is live. Ending the gesture before closing the channel is what keeps that wait
short: the applier folds the `End` in with whatever positions are still queued
and applies none of them.

## 6. Persistence (`tomari-core::db`)

- SQLite (rusqlite, bundled). `Database` wraps a single connection in a
  `Mutex`, with WAL and `foreign_keys = ON`.
- Migrations are `PRAGMA user_version` plus an ordered `MIGRATIONS` list:
  entry `n` upgrades a version-`n` database to `n + 1`, and the version a
  binary writes is simply the list's length. Each step runs in its own
  *immediate* (write-locking) transaction that re-checks `user_version` under
  the lock — two instances racing at launch (the database opens before the
  single-instance guard engages) cannot double-apply a step — and stamps the
  version it reached, so a failure rolls that step back cleanly and the next
  launch resumes from it (covered by tests, including frozen per-version
  fixtures). Shipped entries are never edited; schema changes append a step.
- Tables: `hotkeys` / `modifier_rules` / `settings` (a single `id = 1` row
  holding the `AppSettings` JSON) / `window_placements` (bundle id + one of two
  slot names, with a normalized frame JSON value) / `meta` (app-internal
  key/value records — currently the permission snapshot `regrant.rs` compares
  at launch to detect update-caused revocations — kept out of `settings` so
  they never leak into the settings object the frontend round-trips). Domain
  values are stored as JSON strings in their columns, keeping the schema
  resilient to domain-type evolution.
- First-run seeding keys off the _absence of the settings row_
  (`seed_first_run_defaults` in `main.rs`). Keying off empty tables would
  resurrect defaults whenever a user deliberately clears everything. A launch
  where the seed actually ran is flagged as `AppState::first_run`, which
  `setup` uses to auto-open the settings window once. The frontend pulls it via
  the `setup_status` command (together with the current permission states) to
  open the focused Setup dialog over the current settings page when a permission
  is missing; later recovery opens the same dialog from the sidebar permission
  status. It is a pull, not an event: a push at launch would race the WebView
  load. Any ambiguous detection counts as _not_ a first run, so an
  existing database never triggers it; a corruption reset that re-seeds a fresh
  database does, deliberately — the settings are back at defaults and the
  window shows the user that state. Defaults live in
  `defaults.rs` (Caps Lock → Control — the one seeded modifier rule — plus
  focused window shortcuts for quick snaps, remembered-home restore,
  move-and-restore, undo, and redo). The left/right ⌘ IME toggle is _not_ a
  stored rule: it is assembled on demand from `command_ime_rules` when
  `command_ime_switch_enabled` is on.
- Storage location comes from `AppPaths` (`directories::ProjectDirs`,
  `tomari.sqlite`).
- A *corrupt* database is moved aside under a `.broken-<unix-ms>` suffix and a
  fresh one takes the original path, because for a resident tool losing settings
  beats never starting again; a transient failure (a lock, a read-only or full
  disk) exits with an alert instead, so a healthy database is never discarded.
  `quarantine_database` aims to move the whole set or none of it: a stale `-wal`
  beside a brand-new database is replayed into it, so an incompletely
  quarantined set is worse than the corrupt one it was found as. The database
  moves first, so the common failure — a directory that cannot be written —
  stops before anything has; a sidecar that then cannot be moved rolls back
  everything already moved, leaving the set as found for the next launch to
  retry. That rollback stops at its own first failure rather than pressing on:
  restoring the database while a sidecar stayed aside would leave one that
  *looks* intact to be opened without it. Stopping gives the invariant the sweep
  below relies on — if a rollback failed at all, the database is still aside, so
  whatever the live sidecars happen to be, the next launch sees no database
  beside them. Renaming is the only way files move:
  deleting a sidecar would clear the way too, but it destroys content SQLite
  refused to read (exactly what someone may want to recover by hand) and cannot
  be rolled back.
- Per-file renames are not a transaction, so a crash between two of them — or a
  rollback that cannot finish — can still split the set. That state is
  recognizable, and `sweep_orphan_sidecars` acts on it *before* anything is
  opened: a sidecar with no database beside it is a reset that never finished,
  since SQLite never leaves a WAL without its database. Those get their own
  `.orphaned-<unix-ms>` names, because a fresh database created beside an orphan
  would have SQLite either replay it or delete it, and deleting it takes the last
  copy of whatever it held. Existence is checked through `FileOps::exists`, which
  returns a `Result`: `Path::exists` reports a metadata error as "not there", and
  treating a sidecar we merely failed to look at as absent is how one ends up
  beside the replacement.
- What none of it handles is two launches resetting at once. The database is
  opened *before* the single-instance plugin is registered, so both can find the
  same corruption; migrations survive that race with immediate transactions, this
  reset has nothing equivalent. Nor is the damage confined to the copies kept for
  inspection: the second process can arrive after the first has created the
  replacement and move *that* aside, leaving one process writing to a database no
  longer at the canonical path and the other to a second fresh one — so settings
  saved in that session are gone by the next launch. `rename` replacing its
  destination compounds it. Ruling any of this out needs a lock held across
  processes from before the database is opened, which is a known gap.
- A failed reset exits with an alert naming the files to move by hand, and a
  fresh database that then cannot be created reports *its own* error rather than
  the corruption that started it. The file operations sit behind a `FileOps`
  trait so each failure ordering — a rollback that cannot finish, an orphan that
  cannot be moved, a lookup that cannot be made — is unit tested without a real
  corrupt database.
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
- The activation policy is **Accessory** (no Dock icon). A single resizable
  window (`main`, 940×720 by default, minimum 860×620, decorated, opaque, not
  always on top) is declared in `tauri.conf.json`; a fixed-width sidebar lists
  the five direct destinations beside one scrolling content column. The sidebar
  is grouped into Tools (Windows, Keyboard, Menu Bar, Prevent Sleep) and App
  (General), and carries names rather than secondary descriptions or feature
  state badges. The last selected destination is stored in local storage and
  restored on the next open, with Windows as the fallback. At the narrow
  breakpoint the navigation becomes a horizontal strip and the content keeps a
  usable minimum width. English and Japanese are both checked at the default
  size; longer sections scroll within the window. Closing
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
  created without the permission is null and never revives on its own). Every
  transition also emits `tomari:permissions-changed` (`{ accessibility,
  inputMonitoring }`), which updates the centralized sidebar permission status
  and any open Setup dialog without the window needing to be reopened.
- **Tray** (`tray.rs`): setup items for missing permissions (at the very top),
  explicitly named Undo/Redo Window Change recovery actions, live Prevent
  Sleep and menu-bar-icon state, Settings, and Check for Updates. It does not
  expose the preset grid: window placement belongs to a focused-app shortcut
  or the contextual Windows section. Check for Updates opens the single window
  and emits `tomari:check-update`, which switches the UI to General and starts
  the check. The tray is rebuilt as permission state changes. Labels are
  localized (English / Japanese) from the language setting; `System` resolves
  via `NSLocale` and a language change rebuilds the menu. Undo and Redo also
  follow the window-management master switch and live history availability.
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
- **Frontend** (`src/`): `main.tsx` mounts a single `App` whose sidebar opens an
  `WindowView` / `KeyboardView` / `MenuBarView` / `SessionView` /
  `GeneralView` directly; there is no Overview route. Each detail screen pairs
  a one-sentence purpose with explicit state. Master switches wrap their page
  controls in `FeatureContent`: turning a feature off keeps the configuration
  visible but disables interaction. `WindowView` is segmented into Saved
  Positions / Shortcuts / Mouse, `KeyboardView` into Modifier Keys / Shortcuts,
  and `MenuBarView` into Items / Behavior. `FeaturePageHeader`,
  `SegmentedPageNav`, `SettingsList`, `SettingsRow`, and `PermissionStatus`
  provide the shared presentation vocabulary instead of treating all content
  as generic cards. Missing permissions appear once in the sidebar footer;
  first-run/update re-grant flows render `SetupView` as a modal dialog over the
  selected page. Sections are named for what they do (`SessionView` is *Prevent
  Sleep*, matching its tray entry and its own switch). `lib/api.ts` provides
  typed invoke wrappers whose
  argument keys must match the Rust command parameter names; `lib/types.ts`
  mirrors the domain types. `lib/i18n.tsx` holds the typed English/Japanese
  message dictionaries and the `useT` hook; backend commands return ids (e.g.
  `WindowPreset`) and the frontend renders the localized label. `WindowView`
  renders the focused application's current and remembered normalized frames,
  refreshes that context when the panel becomes active, exposes fixed-position
  operation feedback, and owns shortcut editing for every window action without
  restoring the old preset palette. `KeyboardView` owns general keyboard and
  modifier tap actions, including optional remembered-position restore. Both
  reuse `HotkeyEditor`; its `ShortcutRecorder` suspends registered global
  shortcuts (`set_hotkeys_suspended`) while capturing a chord.
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

## 9. Menu bar tidying (`src-tauri/src/menubar/`)

Gather the status items you rarely look at behind a divider and push them off
the edge of the screen until you ask for them — the job Bartender, Ice and
Hidden Bar do.

AppKit offers no API to enumerate or move another app's status item, so hiding
one directly is impossible. What is possible is to own an item and make it
enormous: the menu bar lays items out right to left, so an item stretched to a
sentinel width (`10_000pt`, which macOS clamps to something a little over the
screen) pushes everything to its left past the edge. **Which icons those are is
the user's own ⌘-drag arrangement** — the app cannot do the sorting for them.

The settings panel can still *inspect* that arrangement. `inventory.rs` asks
each running process for its Accessibility `AXExtrasMenuBar`, reads the child
items' frames and classifies them relative to Tomari's divider. The divider is
expanded only for the scan and restored to the latest live state immediately
afterward. Item ids are snapshot-local: AX exposes neither a durable status-item
identity nor a supported move operation, and item names vary in quality across
applications. The physical ⌘-drag layout therefore remains the single source
of truth; the panel is a live inventory, not a second configuration database.

Two status items, with distinct jobs:

- the **divider** is the boundary. It shows a mark so it can be found and
  ⌘-dragged, and it is the item that stretches.
- the **controller** is the handle: fixed width, always clickable. It must sit
  to the *right* of the divider, or collapsing sweeps the only way back out of
  reach. macOS adds each new item to the left of the existing ones, so the
  controller is created **first** — the order in `make_items` is load-bearing on
  a first run, after which the autosave names take over.

Layered like the rest: `state.rs` is pure (expanded/collapsed, the
auto-collapse deadline, and a generation so a timer armed by one expand cannot
collapse a later one) and unit-tested; `status.rs` is the thin AppKit layer,
main-thread-only in a `thread_local!` with the same last-writer-wins generation
`overlay` uses, since `run_on_main_thread` only queues.

Auto-collapse defaults to **off**. A collapse landing while the user has one of
the revealed menus open would take it away mid-use, which is not a default worth
shipping. State is runtime-only and starts collapsed, except when the feature is
first switched on: that starts expanded, or a user who has arranged nothing yet
sees no effect at all and concludes the switch is broken. Quit removes the items
— not as a safety net (a status item belongs to the process and goes with it,
even on a crash, so there is no `keepawake`-style marker to keep) but so the
menu bar is tidy the moment Tomari is asked to leave.

Verified on macOS 26: the divider stretches, the controller stays on screen, and
clicking it, the tray item, a hotkey or the panel all round-trip. Position
persistence (`autosaveName`) is best effort on Apple's side and is only written
once the user actually drags an item, so a rearrangement surviving OFF→ON and
relaunch is still unproven.

## 10. Permission model

| Permission       | Required by                                        | Acquisition                                                                                                                            |
| ---------------- | -------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| Accessibility    | Moving windows (AX), key synthesis (`keysend`), reading menu bar items | `AXIsProcessTrustedWithOptions` (with prompt)                                                                               |
| Input Monitoring | The keyboard tap and the drag-to-snap tap          | `CGRequestListenEventAccess`. Attempting to create a tap without it adds Tomari to the Input Monitoring list so the user can enable it |
| Administrator    | Keep-awake's lid-close veto (`pmset disablesleep`) | macOS auth dialog via `osascript … with administrator privileges`; required to turn Keep Awake on — the lid-close veto is part of the switch, so declining cancels it |

Global shortcuts need neither permission. The pure engines are testable
without permissions too (unit tests).

## 11. Testing

- **Rust**: pure logic (the engine, geometry, accelerators) is
  covered by in-module unit tests. The DB opens in memory; tests cover the
  migration chain creating the full schema and upgrading frozen fixtures of
  every past schema version to the latest. Window operations are tested
  without the OS via `MockWindowManager`.
- **Frontend**: Vitest + Testing Library (jsdom). `vitest.setup.ts` mocks the
  Tauri API.
- **Toolchain**: clippy (`-D warnings`) / oxlint (type-aware) for linting,
  rustfmt / oxfmt for formatting, tsc for type checking, cargo-deny for
  dependency auditing. `make check` runs the whole local suite. oxlint also
  loads React Doctor's rules (`oxlint-plugin-react-doctor`) via
  `.oxlintrc.react-doctor.json`.
- **CI** (GitHub Actions): four jobs — frontend (ubuntu), Rust tests (macos),
  cargo-deny (ubuntu), and an unsigned macOS debug bundle build (`tauri build
  --debug`) that exercises the same `tauri.conf.json` bundle config a release
  build uses, so a broken bundle setting fails on every push instead of first
  showing up on a tag. Release tags additionally re-run the frontend and Rust
  jobs before publishing (`.github/workflows/release.yaml`), so a release
  build is gated on the same checks as a regular push.

## 12. Adding a feature

1. If it needs domain types and persistence, add the types to `tomari-core`
   and append one migration step to `MIGRATIONS` (keep existing rows alive
   with additive defaults) plus a frozen fixture of the new schema version;
   the schema version follows the list's length automatically.
2. Put decision/computation logic in a new `tomari-<feature>` crate (or an
   existing one) as **pure functions / pure state machines**, with unit
   tests. Isolate OS dependencies behind a trait or a `cfg(target_os)`
   module.
3. If users trigger it, add one variant to `AppAction` and one branch to
   `actions::dispatch`. That alone makes it reachable from hotkeys, taps,
   the tray, and the UI.
4. UI work is a section under `src/views/`, an entry in `SECTIONS` plus an icon
   in `components/icons.tsx`, and additions to `lib/api.ts` / `lib/types.ts`.
   Add a thin Tauri command in `commands.rs` and register it in the handler
   list in `main.rs`. Only add an `api.ts` wrapper for what the panel actually
   calls — an action better driven by hotkey than by mouse (moving a window
   across displays, undo) needs no wrapper at all.
5. In save commands, remember to sync persistence with live state (engines,
   shortcut registration, taps). Restart a tap only when the change truly
   requires it.
