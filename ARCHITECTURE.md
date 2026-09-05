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
│                 (one window, six direct     │
│                  sidebar destinations:      │
│                  Windows / Keyboard /       │
│                  Menu Bar / Prevent Sleep / │
│                  General / Diagnostics)     │
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
| `tomari-keyboard`          | `accelerator`, persisted hotkey/modifier-rule validation and canonicalization, and `ModifierEngine` (tap/hold detection). All pure                                                                            |
| `tomari-window`            | `geometry` (pure preset → frame computation), the `WindowManager` / `WindowHandle` traits plus `MockWindowManager` for tests, and `macos` (the Accessibility API implementation, `cfg(target_os = "macos")`) |
| `src-tauri` (`tomari-app`) | The menu-bar-resident Tauri v2 app. Tray, Tauri commands, global shortcuts, CGEventTap, action dispatch, and sanitized runtime diagnostics                                                                 |
| `src/`                     | React 19 + TypeScript window UI (pnpm workspace, Vite build)                                                                                                                                                 |

Dependencies point one way: `src-tauri` → `tomari-keyboard` / `tomari-window`
→ `tomari-core`. `tomari-core` and `tomari-keyboard` never touch OS APIs.
`tomari-window`'s macOS dependency is isolated in its `macos` module; on other
platforms `MockWindowManager` is plugged in instead (`make_window_manager` in
`main.rs`).

`src-tauri/src/diagnostics.rs` is a read-only aggregation boundary. OS-facing
modules expose only health enums, booleans, counters, and aggregate counts; the
aggregator reads cached Menu Bar permission/divider flags and never starts an AX
inventory scan. The same DTO drives the Diagnostics screen and the versioned
support bundle, preventing the export path from quietly gaining raw input, AX
labels, process details, configured shortcuts or actions, database rows, error
prose, or local filesystem paths.

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
  │     └─ solo tap completed ─► AppAction
  │           ├─ SwitchIme / SendKeystroke: posted here, through the tap proxy
  │           └─ everything else ─► main thread ─► actions::dispatch
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
Every `hidutil` call runs under a deadline (`src-tauri/src/childproc.rs`) so a
wedged one cannot hold a save, the wake reset or quit; a restore that fails on
quit leaves the claim record for the next launch to retry, and the settings
panel reads the live `apply_warnings` (`get_apply_warnings`) when it opens so
that mismatch is shown without waiting for a save.
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
`pending [usage] plan src:dst ...` (write-ahead: the OS write is unconfirmed,
and the record names the whole list it set out to write), or `held [usage]`
(confirmed, and the optional usage is what was displaced for the release to
restore). A `pending` claim over a live Caps Lock → F18 is resolved by that
plan: a live list equal to it means the write landed and only the confirm was
lost, so the next reconcile confirms the claim — or, in the release direction,
hands the source back with the displaced mapping restored — exactly as if the
confirm had succeeded. A write-ahead whose commit fails is retracted on the spot
when nothing of ours can have landed — the write was never handed to `hidutil`,
or it was and our entry is not live right afterwards — since a plan left behind
could otherwise match an identical list the user sets later. A live
Caps Lock → F18 in any other list is unattributable, and then nothing moves: the list, the record and the
`capsLockRemap` warning all stay until the user resolves it (the tap keeps
treating F18 as Caps Lock meanwhile, since that is what is live). The claim is
never quietly dropped over a live remap, which would leave Caps Lock stuck on F18
with the warning gone. Every transition fails closed: a record that cannot be
written, or on the way back cannot be read, aborts it rather than risk the user's
mapping. Taking over always rewrites the record, so a stale one left by a crash
is replaced rather than deleted; every release is gated on our entry still being
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
- When a modifier tap's action *is* synthesized input (`SwitchIme`,
  `SendKeystroke`), the tap posts it itself, from inside the callback, through
  the tap proxy (`CGEventTapPostEvent`; `keysend::Sink::Tap`). That is the one
  place the ordering the user relies on can be guaranteed: the events enter the
  stream at the tap's own position, ahead of every event not yet through it, so
  the character typed a few milliseconds after a ⌘ tap cannot overtake the IME
  switch. Handed to any other thread — the main thread especially, which also
  serves AppKit, the webview and every Accessibility round-trip — the switch
  could land after that character, which would reach the old input method.
  Both key events are built and tagged before either is posted, so a failed
  allocation never leaves an app holding an unmatched key-down. Downstream the
  pair lands just before the modifier release the callback then returns; the
  events carry explicit flags, so apps and the input method read them as plain
  keys. Posting still needs the Accessibility grant (the proxy is no way around
  it, and a refused post is silent), read off an atomic the permission poller
  and `restart` keep current — never from TCC inside the callback. Actions that
  need AppKit (the panel, the menu bar, window ops with their tray refresh)
  are queued on the main thread as before, and a failure to queue is logged
  rather than discarded. Hotkeys, the tray and URLs post at the HID level
  (`Sink::Hid`) as they did.
- (Re)starting the tap is centralized in `eventtap::restart`, called when the
  feature is toggled or the permission is granted. Every teardown — restart,
  master switch off, wake, the system disabling the tap, and `teardown` on quit
  and before the updater's relaunch — first runs `release_held_remaps`: a
  remapped modifier's down was rewritten into its target (Control→Command sent
  the app a Command down), and once the tap is gone the physical release arrives
  as a plain Control up, leaving the app holding Command until it is pressed for
  real. The engine records each held key's remap role *at the press*
  (`ModifierEngine::remap_for` answers from that record while the key is held),
  so a rule edited or removed mid-hold changes neither the rewrite of the
  release nor what is owed. For each owed target (`held_remap_targets`) a
  `flagsChanged` clearing it is synthesized — current combined flags, device
  bits kept, minus the target's generic and device bits, marked synthetic —
  before the engine's hold state is reset; a target whose own key is physically
  down (per the HID system state, which unlike the combined session state does
  not reflect the tap's own rewrite) is left for its real release. A duplicate release, if the new tap
  rewrites the physical up as well, is inert. Without the Accessibility grant
  nothing can be posted; the hold is forgotten and the imbalance logged. The scaffolding underneath
  all three CGEventTaps (this one, drag-to-snap, drag-to-move) — spawn a
  dedicated thread, create the tap, attach its run-loop source, and hand the
  running `CFRunLoop` back so `Drop` can stop and join it, plus re-arming after
  `TapDisabledByTimeout`/`TapDisabledByUserInput` — is itself centralized in
  `tap::spawn`/`tap::reenable` (`src-tauri/src/tap.rs`); only the tap name,
  `CGEventTapOptions`, watched event types, and the callback differ per tap.
  Each tap also keeps a `tap::TapHealthCell` — `Stopped / Starting / Healthy /
  DisabledByTimeout / PermissionDenied / Failed` plus disable/recovery counters,
  logged on every change and never carrying input — and `is_running` reads that
  state rather than the presence of a handle. The permission poller rebuilds
  the taps on *every* Input Monitoring transition, so a revoke lands as
  `PermissionDenied` (and the `keyboardTap`-style warnings) instead of a handle
  to a tap the system no longer feeds.
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

Both keyboard channels accept persisted rows only through the pure
`tomari_keyboard::validation` boundary. It runs on the complete collection at
startup and every hotkey or modifier-rule reload; a save validates the complete
post-upsert collection before writing. Intrinsic validation trims and bounds ids
and labels, canonicalizes accelerator and `SendKeystroke` syntax, rejects
modifier-free global shortcuts other than function keys, and excludes malformed
keystrokes, built-in rule ids and Command-key slots, and rules that combine
Hyper with a remap. Collection validation detects canonical-id collisions,
duplicate accelerators, and duplicate modifier slots. Every member of a
collision is quarantined, independently of row order, and disabled rows still
participate so enabling one cannot silently displace another. Only canonical,
accepted rows reach the shortcut map or modifier engine; the built-in left/right
Command IME rules are appended after persisted-rule validation.

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
   history stores. `set_frame` writes position → size → position and is
   all-or-nothing: the sequence stops at the first write that fails, the writes
   already applied are undone in reverse, and the error
   (`Error::PartialApply`) names the failed step, its cause and the rollback's
   own outcome — so a rollback that fails is reported, never swallowed, and
   callers classify by the root cause (`Error::root`). The macOS implementation is `AxWindowManager` (direct
   bindings to the stable HIServices C functions). Focused-window resolution
   normally reads the system-wide `AXFocusedApplication`/`AXFocusedWindow`, but
   if that application turns out to be Tomari itself (e.g. a click landed on
   the settings window), it falls back to the applications behind us, taken
   front to back from the on-screen `CGWindowList` and asked in turn until one
   answers with a focused window — owning a normal-level window does not mean
   exposing one through Accessibility, and one that does not must not make the
   whole lookup fail. So an operation triggered from Tomari's own UI never
   targets Tomari's own window, and "there is no window to act on" reaches the
   UI as that sentence rather than as a bare AX error code. `FocusedWindow` also
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
the first drag that actually moves its frame the drag arms.

Listen-only means the tap cannot *modify* events, not that it stays out of the
way: the system still holds the event until the callback returns, which is why a
slow one gets the tap disabled by timeout. So this callback obeys the same rule
as the active drag-to-move one — no call into another process, no unbounded
wait. It reads the event's location and flags, checks the `ENABLED` atomic
mirrored out of the settings by `restart_result`, posts a command into a
bounded, coalescing queue (`src-tauri/src/mailbox.rs`: the newest cursor
position replaces the pending one for the same gesture and a cap refuses
samples of further gestures beyond that; lifecycle commands — press, release,
cancel — travel a lock-free channel and are never shed, and each pending sample
is announced there by a tick so it is handed out after its own press and before
its release; a cursor sample that finds the slot's
lock contended is dropped rather than waited for, so the callback never blocks
on the worker; anything shed is counted and logged from the worker) and
returns.
Everything with an Accessibility round-trip in it — the hit-test, the frame
reads that separate a window drag from a text selection, the snap on release —
runs on a single worker thread (`tomari-dragsnap-apply`) started with the tap.
Commands carry the generation of the press they belong to and the queue is
drained before every step, so a press superseded while a call was in flight is
folded into the intent and dropped without a call of its own; the decision of
what to do next (`next_step`) is pure and unit-tested. A press released without
the cursor ever leaving it is an ordinary click, and costs no Accessibility call
when the worker sees both together — opportunistic, since a click whose press it
reaches first is hit-tested like any other.

Because the hit-test no longer runs inside the mouse-down, it is only truthful
while the window is still where it was pressed: a press the worker could not
reach within `PRESS_FRESHNESS` is dropped rather than hit-tested against
coordinates that may by now sit over a different window — one missed snap is
better than snapping the wrong window and recording that in the undo history. A
release is exempt from the frame-read throttle exactly once, so a drag quick
enough to finish inside one interval can still arm, and a window that never
moved is not asked again.

Blocking that callback was not merely a latency cost. With macOS *three-finger
drag* enabled the trackpad synthesizes the left button from finger movement, so
the beginning of a four-finger swipe arrives at this tap as a mouse-down — and a
hit-test holding that event long enough stopped WindowServer from recognizing the
swipe, which surfaced as Mission Control's space-switching gestures
intermittently not working while Tomari ran.

Edge detection needs
each display's full frame and work area, which only the main thread can read
(`WindowManager::screens_cg`) — so that geometry is **cached** in `AppState`
(primed at startup and refreshed whenever the displays change, via the
`NSApplicationDidChangeScreenParametersNotification` observer in `displays.rs`)
and the worker reads the cache on arm, never blocking on a main-thread
round-trip. The cache carries a generation that advances on every refresh: a
preview re-reads the geometry when the generation has moved on, and the drop is
always decided against a snapshot taken under the window-mutation lock, right
before the write (so a change that lands while waiting for that lock is seen
too) — re-targeted
from the last cursor position when the displays changed, aborted when that
position selects no zone any more — so a display unplugged, rearranged or
resized mid-drag (or a Dock that moved) never places the window against
geometry that is gone. Only a change landing during the AX write itself is
beyond that check.
Before the Accessibility hit-test, a front-to-back Window Server snapshot lists
the processes owning a surface at the pointer, and each is AX hit-tested in that
order until one yields a window (`pointer_window_owners` → `window_at_point`);
AX is always scoped to a single application, so Tomari's own AppKit
accessibility is never entered from a worker thread, while floating external
windows remain eligible targets. Finding *our own* surface in front stops the
search — a gesture over Tomari's window is not for what it covers.

Trying candidates in order, rather than trusting the frontmost owner, is what
makes the pointer gestures work at all on current macOS: the Dock owns a window
covering the entire display (wallpaper / Stage Manager) in front of every app
window, and it is not flagged as a desktop element, so the window list keeps it.
Answering with the frontmost owner alone therefore returned "the Dock" for every
point on screen, and since the Dock has no accessible element there, both
drag-to-snap and drag-to-move resolved nothing. Only a candidate with nothing at
the point is looked behind: one that answers with a real element it cannot trace
to a window — a menu, the menu bar — still blocks what is underneath.
Armed drags then resolve the target purely from the cursor (`screen_at_cursor` +
`edge_snap_preset`), and only a change of target (preset _and_ display) touches
the preview. The preview is a translucent, click-through `NSPanel` in
`overlay.rs` — created lazily and held in a main-thread `thread_local!`, since
AppKit windows are not `Send` — driven from the worker through
`overlay::show` / `hide`, which hop to the main thread. On release the window
snaps to the zone the newest cursor position selected — the mouse-up's own
coordinates are deliberately ignored, so a release that drifts out of the edge
band still lands where the drag pointed — and the move is recorded for undo.
That is normally the zone on screen; when a release folds in with the drag
before it, the preview for that last position can be superseded before it
renders, so the drop follows the cursor rather than the last frame the user saw.
A lost mouse-up (tap disabled by the system) drops the drag and tears down its preview, as does
`SnapTapState::drop`, which cancels the press, closes the channel and joins the
worker on the tap thread as it shuts down — before `RunningTap::drop`'s own
thread join returns, so normally no Accessibility call from a torn-down tap is
still in flight once the next tap is live. That wait is short because an
in-flight call is bounded by the AX messaging timeout (an element whose bound
cannot be set is refused outright rather than used unbounded), but `RunningTap`
detaches a tap thread that overruns it — so, as with drag-to-move, the
guarantee is "normally", not "always".
The frame read when the press was resolved is retained as the history entry's
`before` value, so Undo returns to the drag origin rather than the temporary
screen-edge frame at release.
`overlay` gives every issued `show`/`hide` a fresh generation and applies a
queued operation on the main thread only while its generation is still the
current one — last writer wins, with no assumption about delivery order — so a
stale `show` still queued when the tap is torn down can never resurrect a
preview after the teardown's `hide`, nor can a stale `hide` clear a newer one.

**Drag-to-move & resize** (`drag_to_move.rs`) is a third CGEventTap, opt-in and
modifier-gated. Unlike drag-to-snap it does not watch the OS move a window — it
_drives_ the window itself, so it is an **active** tap (`CGEventTapOptions::Default`),
whose callback holds up **all** input while it runs, Tomari's own or not. So the
callback calls into no other process, starts no thread, joins none and waits on
nothing unbounded: it reads the held modifiers (`gesture_for_flags`: `⌃⌥` → move, `⌃⌥⌘` →
resize, Shift up) plus two atomics — `ENABLED`, mirrored out of the settings by
`restart_result`, and `ACCESSIBILITY`, mirrored from the permission poller —
then posts a `Command` into the same kind of bounded, coalescing queue and
returns.

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
  `Mutex`. Opening first requires `PRAGMA quick_check(1)` to return exactly one
  `ok` row; only then are WAL, `foreign_keys = ON`, and migrations allowed to
  touch the file. This catches damage in current-schema table pages that would
  otherwise surface only during a later settings query.
- Migrations are `PRAGMA user_version` plus an ordered `MIGRATIONS` list:
  entry `n` upgrades a version-`n` database to `n + 1`, and the version a
  binary writes is simply the list's length. Each step runs in its own
  *immediate* (write-locking) transaction that re-checks `user_version` under
  the lock — so even two processes at the same database cannot double-apply a
  step — and stamps the version it reached, so a failure rolls that step back cleanly and the next
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
- First-run seeding requires both the _absence of the settings row_ and no rows
  in any persistent table (`seed_first_run_defaults` in `main.rs`). The schema
  and SQLite `user_version` do not count as data, but shortcuts, modifier rules,
  internal metadata, and remembered window placements do. This prevents a
  partial or older store from being mistaken for a pristine install while the
  settings row remains the initialized marker for users who deliberately clear
  their configurable rows. A launch where the seed actually ran is flagged as
  `AppState::first_run`, which
  `setup` uses to auto-open the settings window once. The frontend pulls it via
  the `setup_status` command (together with the current permission states) to
  open the focused Setup dialog over the current settings page when a permission
  is missing; later recovery opens the same dialog from the sidebar permission
  status. It is a pull, not an event: a push at launch would race the WebView
  load. Any ambiguous detection counts as _not_ a first run, so an existing or
  recovered database never triggers it. A quarantined database is kept unseeded
  behind the recovery interlock until the user explicitly resets it; that reset
  relaunches with automation still off. Defaults live in
  `defaults.rs` (Caps Lock → Control — the one seeded modifier rule — plus
  focused window shortcuts for quick snaps, remembered-home restore,
  move-and-restore, undo, and redo). The left/right ⌘ IME toggle is _not_ a
  stored rule: it is assembled on demand from `command_ime_rules` when
  `command_ime_switch_enabled` is on.
- Storage location comes from `AppPaths` (`directories::ProjectDirs`,
  `tomari.sqlite`).
- A *corrupt* database is moved aside under a `.broken-<unix-ms>` suffix and a
  fresh, unseeded one takes the original path, because for a resident tool
  preserving a recoverable copy beats either never starting again or silently
  enabling defaults. The replacement enters a fail-closed recovery session;
  transient open failures (a lock, a read-only or full disk) exit with an alert
  instead, so a healthy database is never discarded. Before the first rename,
  Tomari creates and flushes a `database-reset-required` write-ahead marker in
  the data directory. Its presence wins over first-run detection on every later
  launch, including a crash after the old database moved but before the
  replacement was created. Only a successful, explicitly confirmed
  `DatabaseReset` transaction may remove it; a failed removal prevents relaunch
  and leaves the interlock armed.
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
- Two launches resetting at once are ruled out by the `InstanceCoordinator`
  (`instance_lock.rs`). It takes an advisory `flock` on `tomari.lock` before the
  database or Tauri builder, then binds the activation listener before declaring
  that process primary. The endpoint is `instance.sock` inside a UID-named 0700
  directory below Darwin's `_CS_DARWIN_USER_TEMP_DIR`; every newly published
  socket is verified as 0600. Stale cleanup checks type, ownership, and dev/inode
  stability. An active pre-bind, symlink, regular file, or non-owned endpoint is
  never removed. After the data lock has been won, a disconnected socket owned
  by the current UID can be retired even if a crash interrupted the original
  bind before its mode was tightened.
  A secondary connects with a bounded timeout, authenticates the server with
  `getpeereid`, sends only a fixed activation token, and exits only after an
  exact ACK. The listener authenticates the client UID before reading and never
  accepts cwd, argv, URLs, or other external payload. Requests received before
  Tauri setup are coalesced and delivered after the AppHandle is attached. On
  terminal shutdown the listener stops and its exact dev/inode socket is
  unlinked first, while `flock` remains held until all process-external cleanup
  finishes. A crash releases the lock and leaves a disconnected socket that the
  next lock winner can safely identify. Quarantine renames go through
  `renamex_np(RENAME_EXCL)`, so a `.broken-`
  or `.orphaned-` name that is already taken fails the move atomically instead
  of replacing what an earlier reset kept. A two-process test spawns the test
  binary against the same directory to pin contention and ACK-before-exit down.
  Boundary tests cover fixed-frame parsing, truncated and timed-out clients,
  peer-credential failure on both sides, first-launch races, hostile pre-binds,
  exact modes, pending setup delivery, and listener/lock teardown ordering.
  The credential boundary isolates different local users. A hostile process
  already running under the same UID has equal authority over that user's Unix
  sockets and is explicitly outside the endpoint's security guarantee.
- A failed reset exits with an alert naming the files to move by hand, and a
  fresh database that then cannot be created reports *its own* error rather than
  the corruption that started it. The file operations sit behind a `FileOps`
  trait so each failure ordering — a rollback that cannot finish, an orphan that
  cannot be moved, a lookup that cannot be made — is unit tested without a real
  corrupt database.
- After open and first-run detection, startup reads every required column in
  `settings`, `hotkeys`, `modifier_rules`, `meta`, and `window_placements`
  under one SQLite read transaction before constructing any input or window
  effect. A settings JSON error, a hard query/schema/scalar failure in any
  table, or an inconsistent store with any persisted data but no settings row
  builds `AppState` with `AppSettings::fail_closed()`, an empty modifier engine, an
  empty shortcut map, and `ConfigurationRecovery` instead. Global shortcuts,
  keyboard/drag taps, Caps Lock remapping, menu-bar automation, login-item
  writes, and permission-triggered tap restarts are not attempted in that
  session.
- A readable keyboard row that fails semantic validation does not trigger that
  process-wide recovery state. It stays unchanged in SQLite and remains visible
  through the list commands so the panel can identify it, but it is quarantined
  from every live reload. Other valid rows continue operating. This also covers
  records written by older releases or edited directly in SQLite; runtime safety
  never depends on the current UI having produced every row.
- Recovery is an explicit process boundary. `get_settings` identifies
  retryable failures with `settingsRecoveryRequired` and quarantined databases
  with `databaseResetRequired`; ordinary configuration mutations reject, and
  the frontend mounts only the dedicated recovery view. **Try Again** re-reads
  the complete startup snapshot without modifying SQLite. `DatabaseReset`
  exposes only Reset: only the destructive action the user explicitly confirms
  may retire its write-ahead marker. **Reset** writes the fixed fail-closed profile;
  a settings-only decode failure preserves shortcuts and rules only after every
  stored row is proven readable. Skipped JSON or invalid scalar values in those
  automation rows escalate the same reset to a transactional replacement with
  known-good defaults. Metadata and placements are hard-read before the first
  write; structural failures abort or roll the transaction back, so Reset
  cannot modify a partial store and then relaunch into the same recovery. A
  quarantined fresh DB is seeded the same way. Valid window placements remain
  when their database survived.
  Recovery ownership and terminal intent share `AppLifecycle`'s mutex. Only the
  first retry/reset executes its database closure; duplicates perform no reads
  or writes. That owner first flushes a separate
  `show-panel-after-recovery` filesystem intent; this does not alter the saved
  configuration Retry is inspecting. A quit requested during that bounded
  operation is deferred: a successful first-winner repair completes cleanup and
  relaunches, while a failed repair completes the pending quit without an
  intervening retry. A quit or updater relaunch that already won can never be
  turned into a later recovery restart. The next healthy process shows the
  panel and consumes the intent only after the show succeeds. Automation remains
  off after a Reset until the user deliberately enables it; Try Again respects
  the repaired persisted switches.
- Settings, hotkey, and modifier-rule mutations hold
  `AppState::config_mutation`, so they serialize and the in-memory engines never
  disagree with disk. The recovery gate is checked before and after that lock,
  preventing a stale autosave from queueing behind Reset and landing on the
  repaired database afterward. Remembered-position edits stay on the
  main-thread `window_mutation` coordinator to avoid the shortcut-registration
  deadlock; each entry checks the immutable recovery gate before touching DB or
  history.

## 7. Tauri shell and the frontend boundary

- `main.rs` is the assembly point: resolve the data directory and start
  logging (stderr plus a daily-rotated file under `<data_dir>/logs`, seven days
  kept, each day soft-capped at 8 MiB by `logcap` — seeded from what an
  earlier run wrote that day; past the cap one notice is written and the rest
  of the day's lines go to stderr only) → start the `InstanceCoordinator` (a
  launch that cannot acquire its lock performs authenticated hand-off and exits
  before touching the database) → open the DB and
  preflight the complete startup configuration → build either normal or
  fail-closed `AppState` (DB, both engines, the `WindowManager`, the settings
  cache, the shortcut map, the undo history) → wire the plugins (deep-link /
  autostart / updater / global-shortcut) and the tray → start only the effects
  allowed by the trusted startup plan. The coordinator's listener is already
  live before setup; setup attaches its panel-activation handler before building
  the tray, so a racing second launch is queued rather than lost. macOS reopen
  events surface the panel through the same lifecycle gate. Deep links remain on
  the dedicated OS/plugin channel and are never forwarded through instance IPC
  or parsed from argv.
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
- When `get_settings` reports either recovery code, the frontend does not mount
  the sidebar, permission subscriptions, Setup dialog, or any feature view. It
  shows a focused, localized safety-interlock screen with the paused effect
  list and a two-step reset confirmation. Retryable failures also offer a
  non-destructive retry; `databaseResetRequired` explains the quarantine and
  exposes only Reset. In-flight settings events and apply-warning reads cannot
  dismiss the interlock. Escape cancels reset confirmation and focus returns
  to its trigger. This state is a full-screen workflow rather than a toast
  because no ordinary control is safe to use until the process relaunches from
  a verified configuration.
- **Permission polling**: Accessibility / Input Monitoring change in System
  Settings, outside the app, so a tracked worker runs only the cheap status
  checks every two seconds while a grant is missing and every 30 seconds once
  both are stable. It rebuilds the tray menu on the main thread only on a
  change. When Input Monitoring is newly granted, the dead taps are restarted
  (a tap
  created without the permission is null and never revives on its own). Every
  transition also emits `tomari:permissions-changed` (`{ accessibility,
  inputMonitoring, revision }`), which updates the centralized sidebar
  permission status and any open Setup dialog without the window needing to be
  reopened. The `revision` is a monotonic stamp shared with the `setup_status`
  pull: the frontend registers the listener first, pulls only once it is in
  place, and applies whichever snapshot is strictly newer — so a transition
  landing during the pull is neither lost nor overwritten. Until a snapshot has
  arrived the permissions are *unknown*, shown as "checking" (never as ready),
  and a failed pull leaves a retry on the status control.
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
  `reload_engine_rules`. Hotkey and modifier-rule saves return the canonical
  stored row; the frontend replaces the submitted row with that response so an
  ID trimmed during validation is used by every subsequent edit or delete.
  Tauri runs a synchronous command on the main thread,
  so the hotkey and modifier-rule commands — which reach the database (a
  `SQLITE_BUSY` wait of up to five seconds) and orchestrate a global-shortcut
  re-registration — are `async fn`s whose body runs on the blocking pool via
  `off_main`. Shortcut registration is main-thread work whichever thread asks
  for it: `shortcuts::register_all` reads the database on the caller's thread,
  then runs the plugin calls and the dispatch-map update on the main thread as
  one closure — the plugin waits on the main thread while holding its own
  table lock, which the hotkey handler on the main thread also takes, so doing
  the calls piecemeal from another thread could deadlock. Nothing on the main
  thread may wait for `config_mutation` for the same reason. The window
  commands stay
  synchronous on purpose: `WindowManager::work_area` / `screen_work_areas`
  read AppKit screen geometry that is only correct on the main thread, and
  every window operation holds `AppState::lock_window_mutation` for its whole
  run so the panel, the shortcuts and the drag-to-snap worker never interleave
  one (the history's own lock guards a push/pop, not the sequence). Commands
  reject with a `CmdError`
  (`{ code, message }`, `src-tauri/src/error.rs`): the frontend localizes the
  frequent `code`s (missing permission, no focused window, shortcut conflict)
  and falls back to the English `message` for the rest.
- **Configuration warnings**: `AppState::configuration_warnings` holds the
  complete hotkey/modifier quarantine snapshot and increments its revision only
  when visible contents change. The frontend subscribes to
  `tomari:configuration-warnings-changed` before pulling
  `get_configuration_warnings`, then accepts only a strictly newer revision;
  listener failure still leaves the pull as a useful snapshot, and every panel
  show re-pulls both warning channels so a missed event or transient command
  failure self-heals. Warning rows retain the exact raw identity for deletion,
  while all persisted user-controlled display text is stripped of control and
  bidi-format characters and bounded before reaching visible or accessible UI.
  This channel is deliberately separate from `apply_warnings`: configuration
  warnings explain saved records that were not allowed to become live, while
  apply warnings describe otherwise-valid configuration whose OS side effect
  did not apply. The recovery interlock mounts neither warning channel until
  settings are trustworthy.
- **Frontend** (`src/`): `main.tsx` mounts a single `App` whose sidebar opens an
  `WindowView` / `KeyboardView` / `MenuBarView` / `SessionView` /
  `GeneralView` directly; there is no Overview route. Each detail screen pairs
  a one-sentence purpose with explicit state. The master control is a
  `FeatureSwitch` row placed first in the content column (not in the page
  header), so it lines up with every other row control. Hierarchy is carried by
  size: the master toggle is the only regular-size `Toggle`; every subordinate
  option renders the mini size. *Prevent Sleep* is an authorized operation
  rather than a stored preference, so its master control is a verb button
  (Start Preventing Sleep… / Stop Preventing Sleep) and the row also carries the
  live phase, countdown, and the Cancel / Retry actions. The persistent
  features wrap their page controls in `FeatureContent`: turning a feature off
  keeps the configuration visible but disables interaction. *Prevent Sleep*
  deliberately does not — its auto-off conditions stay editable while off so
  they can be set before turning it on. `WindowView` is segmented into Saved
  Positions / Shortcuts / Mouse, `KeyboardView` into Modifier Keys / Shortcuts,
  and `MenuBarView` into Items / Behavior. `FeaturePageHeader`, `FeatureSwitch`,
  `SegmentedPageNav`, `SettingsList`, `SettingsRow`, and `PermissionStatus`
  provide the shared presentation vocabulary instead of treating all content
  as generic cards. Missing permissions appear once in the sidebar footer;
  first-run/update re-grant flows render `SetupView` as a modal dialog over the
  selected page. Sections are named for what they do (`SessionView` is *Prevent
  Sleep*, matching its tray entry). `lib/api.ts` provides
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
  shortcuts (`set_hotkeys_suspended`) while capturing a chord. Quarantined
  keyboard records produce a persistent, localized amber shell banner whose
  action opens and focuses the Keyboard explanation; that view groups the
  affected records and localizes each stable reason code while retaining the
  persisted identity needed to repair or remove the underlying row.
- **Terminal shutdown** (`lifecycle.rs`): `AppState` owns a one-way terminal
  lifecycle with a non-terminal `Recovering` sub-state:
  `Running → Recovering → ShuttingDown → Stopped` on successful repair, or
  `Recovering → Running` when repair fails without a pending quit. Ordinary
  quit holds the first `ExitRequested` on the main thread, marks the lifecycle
  terminal there, and
  runs cleanup on the blocking pool; this leaves the main thread available to
  finish a shortcut registration already in progress. The updater runs the
  same coordinator synchronously before asking Tauri to restart. New config
  mutations, tracked workers, tap/Caps effects, and the Menu Bar synthetic
  drag are rejected once terminal; work that already crossed a gate is drained
  before cleanup continues. The fixed order is: stop the instance activation
  listener while retaining its data lock, cancel and join process-lifetime
  workers, drain transient OS effects, release global shortcuts, stop the
  keyboard and both drag taps, restore native Caps Lock, remove menu-bar UI,
  then release keep-awake state. Process teardown releases the coordinator's
  `flock` only after that cleanup. Concurrent shutdown calls wait for the same
  completion and never reopen the lifecycle; relaunch continuations run only
  for the caller that atomically won the terminal claim.
- **Updater**: `tauri-plugin-updater`. The `Update` found by
  `check_for_update` is held in `PendingUpdate` until `install_update`
  consumes it, completes the terminal shutdown above, and relaunches. The
  endpoint is `latest.json` on GitHub Releases.
- **External control / URL scheme** (`tomari-core::external`,
  `dispatch_deep_link` in `main.rs`): launchers like Raycast/Alfred drive
  Tomari through `tomari://v1/...`. `tauri-plugin-deep-link` delivers URLs; the
  cold-start URL (`get_current`) and warm-start URLs (`on_open_url`) funnel
  through one handler — never argv. The URL itself is never logged (a local
  sender can put anything in it); a refusal logs only the error's kind and an
  accepted action's failure only the action and error code, at most one line
  per five seconds. `parse_deep_link` validates strictly
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
and both directions go through it on the worker thread — which (not the
command) commits the `active` flag. Turning on takes the idle assertion immediately and
shows on; if the veto then cannot be engaged (auth declined, or the sleep state
is unreadable) the whole switch rolls back off. An enable whose read-back fails
is not written off as "off": the flag was clear before the enable, so the worker
reads once more, then clears the override again in the same cycle and confirms
*that*; only when neither can be confirmed is the override recorded as
*possibly* ours (`Ownership::PossiblyOwned`, marker kept) and the switch rolled
back with a `lidCloseUnconfirmed` notice. Retry from that state is a dedicated
recovery: keep-awake is already off, so instead of `disengage` (which would
return at once) it stamps its own transition and runs the clear on the worker;
a clear that cannot be confirmed lands as `RecoveryFailed` — still off, notice
and retry intact — never as a spurious "on", and cancelling it returns to that
same unresolved state rather than reversing into an enable. A possibly-owned override is never
mistaken for a foreign one afterwards: reading it set leaves it possibly ours
(consistent with our enable, not proof of it), and off and exit clear it. A
marker that survives `reconcile_on_launch` (unreadable state, or an override
still set that the marker alone cannot attribute) is carried into the runtime
state the same way, so the new run offers the decision rather than showing a
clean off. Turning off is deferred to the
worker: clearing the override needs an admin dialog that can be declined, and
sleep is still prevented until it succeeds, so a declined clear keeps keep-awake
on. The exported state machine distinguishes `off`, `enabling`, `on`,
`disabling`, and `failed`; all ordinary toggle entry points reject re-entry
during the two pending phases. An explicit cancel terminates the active
`osascript`, bumps a `generation` counter, and queues the reverse reconcile.
That lets a slow worker detect supersession while preserving any ownership it
acquired just before cancellation (the pure `reconcile_writeback` decides the
final state and is unit-tested). A worker that finds itself already superseded
when it wins `LID_OP_LOCK` — the reverse worker, or `cleanup_blocking`, got the
lock first — returns before its first `pmset`, so it can never re-enable the
override the winner just cleared.

Keep-awake is **runtime state** in `AppState` (`Mutex<KeepAwake>`), never
persisted: it always starts off at launch. A toggle reaches it from the tray (a
`CheckMenuItem`), the panel (`get_keep_awake` / `set_keep_awake` commands), and
`AppAction::ToggleKeepAwake` (hotkeys / taps). Every change emits
`tomari:keep-awake-changed` and rebuilds the tray, so the panel's Start / Stop
button and the tray checkmark stay in sync regardless of which surface initiated it. Commands,
reconcile workers, and the safety monitor all emit, and each snapshots under the
state lock but emits outside it, so the events themselves can arrive out of
order. Every snapshot the frontend can receive — reads and events alike —
therefore takes a `revision` under that same lock, so a snapshot issued later
always outranks one issued earlier. The panel treats the event, never a command's
return value, as the state; it subscribes before its first read (an event emitted
in that gap would simply be lost) and drops any snapshot older than one it has
already applied, rather than being stranded on a transition that has since
finished.

A backend monitor keeps session-only safety policy independent of the settings
window. It refreshes AC/battery data, the actual kernel `SleepDisabled` flag,
and a bounded list of known developer processes running for at least five
minutes. It is notification-driven where the system offers a notification — a
power-source change reaches it at once through IOKit
(`IOPSNotificationCreateRunLoopSource`, on its own run-loop thread) — and polls
only as a fallback, at a cadence that follows what there is to do: every ten
seconds while keep-awake is on (or mid-transition) or the panel is showing the
status, every two minutes otherwise, reading only the power source then and
leaving the kernel flag and the process scan (the expensive read) for the next
full pass. Any keep-awake state change or panel show/hide wakes it immediately,
so the panel never opens onto stale data. Absolute auto-off deadlines, AC-only operation, and
the low-battery warn/turn-off policy are evaluated in that order by the tested
pure `safety_decision`, which commits its verdict — the notice and the transition
stamp both, via `begin_disable` — in the same critical section that made it, so
an option edit landing in the gap cannot have a stale verdict applied to it.
Automatic turn-off enters the same administrator-backed disable transition as a
manual request. Because that clear can be declined, a
guard fires at most once per session (`guards_blocked`, re-armed only by an
explicit request in `set` — never by settling back on, which is where cancelling
an automatic turn-off lands), which keeps a decline from reopening the dialog
every tick — but `failed` itself does not disarm the
guards: a session left running by a declined manual off is still holding sleep
off, and is exactly when a deadline or a dying battery must still act. Relative presets are re-armed on every
engage, and an absolute end time already in the past is spent rather than a
reason to refuse: engaging drops it, so the tray item and the global shortcut —
neither of which can edit the deadline — never dead-end on a stale one. The
frontend derives its countdown from the backend deadline and mirrors the options
the backend reports back, so it never enforces or resurrects a deadline itself.

Every clear — the user-confirmed recovery and exit-time `cleanup_blocking` —
goes through the same verified `cleanup_lid_close_with`, so a setter reporting
success without moving the kernel flag never takes the marker with it. Shutdown
also terminates an administrator prompt a worker left on screen before waiting
on `LID_OP_LOCK`, so a quit or an updater restart cannot block on a dialog for
work it is about to undo — and every wait on that path is bounded. The lock
itself is taken with `try_lock` in a loop that keeps killing the dialog (a
worker that spawned its `osascript` just after the first kill is caught by the
next), giving up after `EXIT_LOCK_DEADLINE` (10 s) and leaving the override to
the marker rather than clearing it unserialized. The clear's own administrator
dialog is given `EXIT_AUTH_DEADLINE` (10 s), after which the `osascript` is
killed and reaped and the clear counted as not applied; an interactive worker
that notices shutdown has begun switches to the same bound. Every child that only
reads — `pmset -g`, `pmset -g batt`, the `ps` scan — runs under `READ_DEADLINE`
(5 s), since none needs user input, so one hung child can neither stall exit nor
stop the safety monitor for good. The kernel
flag is read as always, so an override still set keeps its marker and ownership
and the next launch surfaces it; the process is never held hostage to a dialog
nobody is there to answer. The launch reconcile runs in `setup`, possibly
unattended, and therefore shows no dialog at all. Only the interactive toggles
and the user-confirmed recovery wait on the user, who has Cancel.

Because `disablesleep` survives a crash, a marker file under the data directory
records that _we_ engaged it. But the marker is write-ahead evidence that we
_may_ have left the override, not proof that the one set now is still ours: the
kernel flag records no provenance, and the user or another tool may have set it
since the crash. So `reconcile_on_launch` (from `setup`) clears nothing on the
marker alone. A marker whose override is already gone (a reboot cleared it) is
dropped; a marker with the override still set is surfaced as the
`leftoverOverride` notice, and the user decides — "turn sleep back on" runs the
verified clear (with its admin prompt), "leave it as it is" drops the marker and
returns keep-awake to a clean off (verified gone before the state commits).
Until that decision (`leftover_undecided`), exit leaves the override alone and
keeps the marker so the next launch asks again, and the ordinary on/off paths —
panel, tray item, hotkey — are refused, since any of them would end in a clear
justified by the marker alone. An unreadable sleep state at launch with a marker
present is handled the same way under the `lidCloseUnconfirmed` notice.
The terminal lifecycle coordinator invokes `cleanup_blocking` for tray Quit,
updater relaunch, and logout alike, after it has canceled and joined keep-awake
workers. It otherwise releases everything before the process exits. The pure
`reconcile_decision` is unit-tested; the IOKit / `pmset` layer stays thin.

## 9. Menu bar tidying (`src-tauri/src/menubar/`)

Gather the status items you rarely look at behind a divider and push them off
the edge of the screen until you ask for them — the job Bartender, Ice and
Hidden Bar do.

AppKit offers no API to enumerate or directly reposition another app's status
item. Tomari owns a divider and makes it enormous: the menu bar lays items out
right to left, so a divider stretched to a sentinel width (`10_000pt`, which
macOS clamps to something a little over the screen) pushes everything to its
left past the edge. The physical ordering around that divider remains the
source of truth.

The settings panel can inspect and best-effort edit that arrangement.
`inventory.rs` asks
each running process for its Accessibility `AXExtrasMenuBar`, reads the child
items' frames and classifies them relative to Tomari's divider. The divider is
expanded only for the scan and restored to the latest live state immediately
afterward. Disabled Control Center modules remain in the AX tree as zero-area
placeholders, so the scanner rejects non-positive frames. Names prefer a real
title, then the description (with dynamic Control Center details removed), then
a known system menu-extra identifier; generic role labels and owner-only
fallbacks cannot mask a later item-specific name. Transient AX label failures
are retried once; persistent failures omit the item from that snapshot instead
of mislabeling it with its owner's name.
Item ids are snapshot-local: AX exposes neither a durable status-item identity
nor a supported move operation, and item names vary in quality across
applications. Every published inventory receives generation-scoped opaque ids;
a refresh invalidates the preceding generation rather than letting a stale row
target a different item.

`movement.rs` implements the public fallback available to an assistive app: it
posts a short, interpolated mouse drag with the Command flag from the AX frame
to the requested side of the divider. Scans and moves share one session lock;
all other physical divider updates defer until that operation releases it. A
move expands the divider, resolves the opaque id against a fresh scan, refuses
ambiguous identities, and rechecks the selected process, item frame, and divider
geometry immediately before mouse-down. While the synthetic gesture is in
flight a short-lived listen-only tap watches for any input that does not carry
Tomari's synthetic marker — a real click, drag or key — and the gesture cancels
at the next step, the drop guard posting the matching release and restoring the
cursor (to its original point, or the main display's centre if that display has
gone away). Without Input Monitoring the tap cannot start and the gesture falls
back to the button-state checks alone. A stable AX identifier remains valid
when a dynamic display label changes; an item without one must retain its label
and snapshot geometry before Tomari will touch it. Post-drag verification waits
on that retained AX element, takes one full inventory for the UI, then checks
the same element again so it cannot go stale while other applications are
scanned. Every exit restores the pointer and latest live collapsed state, and
only an item confirmed by both views on the requested side is reported as
moved. Items behind a notch or implementations that reject synthetic input
therefore fail closed and keep the manual ⌘-drag as their fallback. Tomari
stores no parallel desired-layout database; the physical arrangement remains
authoritative.

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
| Accessibility    | Moving windows (AX), key and menu-bar drag synthesis, reading menu bar items | `AXIsProcessTrustedWithOptions` (with prompt)                                                                            |
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
  showing up on a tag. Release tags additionally re-run the frontend, Rust and
  cargo-deny jobs before publishing (`.github/workflows/release.yaml`), so a
  release build is gated on the same checks as a regular push — a tag pushed
  from a commit that never went through CI cannot publish an artifact whose
  dependency policy was never checked.

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
