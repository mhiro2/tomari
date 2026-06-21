//! The persistent CGEventTap that connects real keyboard activity to the pure
//! [`ModifierEngine`](tomari_keyboard::ModifierEngine).
//!
//! A dedicated thread owns the tap and runs a `CFRunLoop`, since
//! `CFRunLoopRun` blocks. The callback observes `flagsChanged` / `keyDown` /
//! `keyUp` events, feeds the engines, and:
//!
//! * **remaps** an ordinary modifier (Control/Option/Command/Shift/fn) by
//!   rewriting its `flagsChanged` flags and keycode in place; while it is held,
//!   its target modifier is also stamped onto the keystrokes typed through it so
//!   a chord lands as the target (e.g. Control→Command + C registers as Cmd+C);
//! * handles **Caps Lock** specially: macOS gives it no usable key-up and lets
//!   it lock, so it is first remapped to F18 at the HID level
//!   ([`crate::capsmap`]) and arrives here as F18 key-down/up, which the tap
//!   drives as the Caps Lock modifier (dropping the F18 event). Tapped it fires
//!   its action (e.g. Esc); held it stamps its target (e.g. Control) onto the
//!   following keystrokes;
//! * stamps the **hyper** combo (⌃⌥⇧⌘) onto keystrokes typed while a hyper key
//!   is held;
//! * dispatches a modifier's **tap** action (IME switch, snap, …) on release.
//!
//! Creating the tap requires the *Input Monitoring* permission; if it is not
//! granted the OS returns a null tap (and adds Tomari to the Input Monitoring
//! list so the user can enable it).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use core_foundation::base::TCFType;
use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes};
use core_foundation_sys::mach_port::CFMachPortRef;
use core_graphics::event::{
    CGEvent, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
    CallbackResult, EventField,
};
use tauri::{AppHandle, Manager};
use tomari_core::{AppAction, KeySide, ModifierKey};
use tomari_keyboard::HYPER_MODIFIERS;
use tomari_keyboard::engine::KeyEvent;

use crate::keycodes;
use crate::locks::MutexExt;
use crate::state::AppState;

/// Marker written into `EVENT_SOURCE_USER_DATA` on events Tomari synthesizes
/// (see [`crate::keysend`]), so the tap ignores its own injected keystrokes.
pub const SYNTHETIC_MARKER: i64 = 0x746f_6d72; // "tomr"

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    fn CGPreflightListenEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
}

/// Whether the *Input Monitoring* permission has been granted.
pub fn input_monitoring_granted() -> bool {
    unsafe { CGPreflightListenEventAccess() }
}

/// Prompt for the *Input Monitoring* permission, opening System Settings the
/// first time. Returns whether it is already granted.
pub fn request_input_monitoring() -> bool {
    unsafe { CGRequestListenEventAccess() }
}

/// The single live event tap, owned globally so its lifecycle is independent of
/// the cross-platform [`AppState`] struct.
static EVENT_TAP: Mutex<Option<EventTap>> = Mutex::new(None);

/// Whether Caps Lock is currently remapped to F18 (see [`crate::capsmap`]), so
/// F18 key events are the Caps Lock modifier rather than a real F18 key. Kept in
/// step with the remap by [`restart`]; read on the tap thread for every
/// keystroke, so it is an atomic rather than behind a lock.
static CAPS_PROXY_ACTIVE: AtomicBool = AtomicBool::new(false);

/// A running event tap: the run loop it is attached to (so it can be stopped)
/// and the thread driving it.
pub struct EventTap {
    run_loop: CFRunLoop,
    thread: Option<JoinHandle<()>>,
}

impl Drop for EventTap {
    fn drop(&mut self) {
        // Stopping the run loop makes `CFRunLoopRun` return; the thread then
        // drops the tap (invalidating it) and exits.
        self.run_loop.stop();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

/// (Re)start the tap to match the current settings: tears down any existing tap
/// and, if keyboard customization is enabled, starts a fresh one. Safe to call
/// repeatedly (e.g. when the feature is toggled).
pub fn restart(app: &AppHandle) {
    let mut guard = EVENT_TAP.lock_safe();
    *guard = None; // Drop stops the previous tap.

    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    // The torn-down tap loses the release of any key held across the restart,
    // so drop the engine's transient "key is held" state. Otherwise a held
    // modifier would linger in `held` and the next solo tap would be misread as
    // a chord.
    state.engine.lock_safe().reset();

    if !state.keyboard_enabled() {
        // Feature off: take the Caps Lock HID remap down along with the tap.
        CAPS_PROXY_ACTIVE.store(crate::capsmap::reconcile(false), Ordering::SeqCst);
        return;
    }

    match start(app.clone()) {
        Ok(tap) => {
            *guard = Some(tap);
            tracing::info!("keyboard event tap started");
            // Remap Caps Lock to F18 only now the tap that handles F18 is live,
            // and record the *actual* resulting state (not the request) so a
            // failed `hidutil` cannot leave the proxy flag out of step with the
            // real mapping — which would route F18, or a stuck Caps, wrongly.
            let manage = state.engine.lock_safe().has_caps_lock_rule();
            CAPS_PROXY_ACTIVE.store(crate::capsmap::reconcile(manage), Ordering::SeqCst);
        }
        Err(e) => {
            // No tap to handle F18 — keep Caps Lock native.
            CAPS_PROXY_ACTIVE.store(crate::capsmap::reconcile(false), Ordering::SeqCst);
            tracing::warn!(error = %e, "keyboard event tap not started (grant Input Monitoring?)")
        }
    }
}

/// Reconcile the Caps Lock HID remap (and the F18 proxy flag) with the current
/// rules *without* restarting the tap — for use after a live rule edit, where
/// the tap reads the engine directly but the HID remap must still be brought
/// into step. A no-op unless the tap is running, so it never remaps Caps Lock to
/// F18 with no tap to handle it.
pub fn reconcile_caps_mapping(state: &AppState) {
    let manage = EVENT_TAP.lock_safe().is_some()
        && state.keyboard_enabled()
        && state.engine.lock_safe().has_caps_lock_rule();
    CAPS_PROXY_ACTIVE.store(crate::capsmap::reconcile(manage), Ordering::SeqCst);
}

/// Mutable state local to the tap thread, reached through a `Mutex` because the
/// CGEventTap callback is `Fn`, not `FnMut`.
#[derive(Default)]
struct TapState {
    /// Caps Lock's flag reflects the lock state, not the key, so its down/up
    /// must be tracked by toggling (all other modifiers derive theirs from
    /// the event's own flags).
    caps_down: bool,
    /// Whether *any* hyper key is currently held (tracked off the engine's held
    /// set, so holding two and releasing one keeps hyper active).
    hyper_active: bool,
    /// Modifier flags to `(remove, add)` on keystrokes typed while remapped
    /// keys are held, so a chord through them carries the target modifier. Caps
    /// Lock especially needs this: as a lock key it leaves the OS with no
    /// held-modifier state, so the rewritten `flagsChanged` flag is not carried
    /// onto the following keys (see
    /// [`tomari_keyboard::ModifierEngine::held_remap_stamp`]).
    remap_stamp: (Vec<ModifierKey>, Vec<ModifierKey>),
}

fn start(app: AppHandle) -> Result<EventTap, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("tomari-eventtap".into())
        .spawn(move || run_tap(app, tx))
        .map_err(|e| e.to_string())?;

    match rx.recv() {
        Ok(Ok(run_loop)) => Ok(EventTap {
            run_loop,
            thread: Some(thread),
        }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(e) => Err(format!("event tap thread exited before signalling: {e}")),
    }
}

fn run_tap(app: AppHandle, tx: Sender<Result<CFRunLoop, String>>) {
    let state = Arc::new(Mutex::new(TapState::default()));
    let port_holder = Arc::new(AtomicUsize::new(0));

    let callback = {
        let app = app.clone();
        let state = state.clone();
        let port_holder = port_holder.clone();
        move |_proxy, etype, event: &CGEvent| handle_event(&app, &state, &port_holder, etype, event)
    };

    let tap = match CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::Default,
        vec![
            CGEventType::KeyDown,
            CGEventType::KeyUp,
            CGEventType::FlagsChanged,
        ],
        callback,
    ) {
        Ok(tap) => tap,
        Err(()) => {
            let _ = tx.send(Err(
                "failed to create event tap — Input Monitoring permission required".into(),
            ));
            return;
        }
    };

    // Publish the mach port so the callback can re-arm the tap if the system
    // disables it after a slow callback or heavy input.
    port_holder.store(
        tap.mach_port().as_concrete_TypeRef() as usize,
        Ordering::SeqCst,
    );

    let source = match tap.mach_port().create_runloop_source(0) {
        Ok(source) => source,
        Err(()) => {
            let _ = tx.send(Err("failed to create run-loop source for event tap".into()));
            return;
        }
    };

    let run_loop = CFRunLoop::get_current();
    unsafe {
        run_loop.add_source(&source, kCFRunLoopCommonModes);
    }
    tap.enable();

    let _ = tx.send(Ok(run_loop));
    CFRunLoop::run_current();
    // Run loop stopped: returning here drops `tap`, invalidating the port.
}

fn handle_event(
    app: &AppHandle,
    state: &Arc<Mutex<TapState>>,
    port_holder: &Arc<AtomicUsize>,
    etype: CGEventType,
    event: &CGEvent,
) -> CallbackResult {
    // The system disabled the tap (timeout / heavy input): re-enable it.
    if matches!(
        etype,
        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
    ) {
        let port = port_holder.load(Ordering::SeqCst) as CFMachPortRef;
        if !port.is_null() {
            unsafe { CGEventTapEnable(port, true) };
        }
        return CallbackResult::Keep;
    }

    // Ignore keystrokes Tomari itself synthesized.
    if event.get_integer_value_field(EventField::EVENT_SOURCE_USER_DATA) == SYNTHETIC_MARKER {
        return CallbackResult::Keep;
    }

    let Some(app_state) = app.try_state::<AppState>() else {
        return CallbackResult::Keep;
    };
    let app_state = app_state.inner();

    if !app_state.keyboard_enabled() {
        return CallbackResult::Keep;
    }

    let now = app_state.now_ms();

    match etype {
        CGEventType::FlagsChanged => on_flags_changed(app, app_state, state, event, now),
        CGEventType::KeyDown => on_key_down(app, app_state, state, event, now),
        CGEventType::KeyUp => on_key_up(app, app_state, state, event, now),
        _ => CallbackResult::Keep,
    }
}

/// Derive whether a `flagsChanged` event is a press (`true`) or release
/// (`false`) from the event's flag bits, not from remembered down/up state.
///
/// A tap restart (settings save, permission grant, new `TapState`) can swallow
/// a transition mid-hold; a toggle would then read the eventual release as a
/// press and stay inverted for that keycode from then on. Reading the flags
/// instead keeps a stale state from inverting the interpretation.
///
/// Left/right modifiers carry an IOKit device-specific bit while physically
/// held; Fn tracks its generic flag bit directly. Caps Lock is the lone
/// exception: its AlphaShift bit reflects the lock state, not the key, so its
/// down/up can only be tracked by toggling `caps_down` (wrong only if a restart
/// lands while Caps Lock itself is physically held — rare, as it is tapped).
fn derive_is_down(
    keycode: i64,
    modkey: ModifierKey,
    flags_bits: u64,
    caps_down: &mut bool,
) -> bool {
    if let Some(bit) = keycodes::device_flag_for_keycode(keycode) {
        flags_bits & bit != 0
    } else if modkey != ModifierKey::CapsLock {
        flags_bits & keycodes::flag_for(modkey).bits() != 0
    } else {
        *caps_down = !*caps_down;
        *caps_down
    }
}

/// A managed modifier's down/up transition fed to [`drive_modifier`].
struct ModifierEvent {
    key: ModifierKey,
    side: KeySide,
    is_down: bool,
}

/// Drive a managed modifier's down/up into the engines and apply its side
/// effects: hyper-held tracking, the remap-stamp set applied to later
/// keystrokes, and dispatching a completed tap action.
///
/// Returns the key's held roles `(remap, hyper)` so a caller that passes the
/// originating modifier event through (a real `flagsChanged`) can rewrite it in
/// place. The Caps-Lock-as-F18 path ignores the roles and drops its key event:
/// Caps Lock has no `flagsChanged` to rewrite, so its remap target reaches
/// keystrokes purely through the stamp set this records.
fn drive_modifier(
    app: &AppHandle,
    app_state: &AppState,
    state: &Arc<Mutex<TapState>>,
    ev: &ModifierEvent,
    now: u64,
) -> (Option<ModifierKey>, bool) {
    let ModifierEvent { key, side, is_down } = *ev;

    // Feed the engine and read back this key's held role plus the post-event
    // held set (so the stamp tracking reflects this very up/down).
    let (tap_action, remap, hyper, any_hyper_held, held_remap_stamp) = {
        let mut engine = app_state.engine.lock_safe();
        let action = engine.process(if is_down {
            KeyEvent::ModifierDown {
                key,
                side,
                at_ms: now,
            }
        } else {
            KeyEvent::ModifierUp {
                key,
                side,
                at_ms: now,
            }
        });
        (
            action,
            engine.remap_for(key, side),
            engine.is_hyper(key, side),
            engine.is_any_hyper_held(),
            engine.held_remap_stamp(),
        )
    };

    {
        let mut ts = state.lock_safe();
        // Track whether *any* hyper key is still held, not just this event's
        // direction: releasing one of two held hyper keys must not drop the
        // ⌃⌥⇧⌘ stamp while the other is still down.
        ts.hyper_active = any_hyper_held;
        // Refresh the flags stamped onto later keystrokes from the now-updated
        // set of held remapped keys (this event may have added or removed one).
        ts.remap_stamp = held_remap_stamp;
    }

    if let Some(action) = tap_action {
        dispatch_async(app, action);
    }

    (remap, hyper)
}

fn on_flags_changed(
    app: &AppHandle,
    app_state: &AppState,
    state: &Arc<Mutex<TapState>>,
    event: &CGEvent,
    now: u64,
) -> CallbackResult {
    let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
    let Some((modkey, side)) = keycodes::modifier_for_keycode(keycode) else {
        return CallbackResult::Keep;
    };

    // Derive down/up from the event itself, not remembered state (see
    // `derive_is_down`).
    let is_down = {
        let mut ts = state.lock_safe();
        derive_is_down(keycode, modkey, event.get_flags().bits(), &mut ts.caps_down)
    };

    let (remap, hyper) = drive_modifier(
        app,
        app_state,
        state,
        &ModifierEvent {
            key: modkey,
            side,
            is_down,
        },
        now,
    );

    // A hyper key contributes its combo to later keystrokes, not to its own
    // event — strip the source flag so e.g. Caps Lock does not toggle.
    if hyper {
        let mut flags = event.get_flags();
        flags.remove(keycodes::flag_for(modkey));
        event.set_flags(flags);
        return CallbackResult::Keep;
    }

    // Remap: rewrite the event to the target modifier in place.
    if let Some(target) = remap {
        let mut flags = event.get_flags();
        flags.remove(keycodes::flag_for(modkey));
        if is_down {
            flags.insert(keycodes::flag_for(target));
        }
        event.set_flags(flags);
        event.set_integer_value_field(
            EventField::KEYBOARD_EVENT_KEYCODE,
            keycodes::primary_keycode(target),
        );
    }

    CallbackResult::Keep
}

fn on_key_down(
    app: &AppHandle,
    app_state: &AppState,
    state: &Arc<Mutex<TapState>>,
    event: &CGEvent,
    now: u64,
) -> CallbackResult {
    let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);

    // Caps Lock arrives here, as F18, only because we remapped it at the HID
    // level (see `capsmap`) — that is the only way to get a real key-down/up for
    // it and to stop it locking. Drive it as the Caps Lock modifier and drop the
    // F18 event so no app sees a stray F18. Autorepeat keyDowns during a hold are
    // dropped without re-driving, so they neither re-fire side effects nor reach
    // the app while the matching keyUp is still dropped.
    if keycode == crate::capsmap::F18_KEYCODE && CAPS_PROXY_ACTIVE.load(Ordering::SeqCst) {
        // Autorepeat key-downs during a hold neither re-fire side effects nor
        // reach an app; only the initial press drives the modifier down.
        if event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT) == 0 {
            drive_modifier(
                app,
                app_state,
                state,
                &ModifierEvent {
                    key: ModifierKey::CapsLock,
                    side: KeySide::Either,
                    is_down: true,
                },
                now,
            );
        }
        return CallbackResult::Drop;
    }

    // Any non-modifier key turns a pending modifier tap into a chord.
    app_state
        .engine
        .lock_safe()
        .process(KeyEvent::OtherKeyDown { at_ms: now });

    stamp_held_modifiers(state, event);
    CallbackResult::Keep
}

fn on_key_up(
    app: &AppHandle,
    app_state: &AppState,
    state: &Arc<Mutex<TapState>>,
    event: &CGEvent,
    now: u64,
) -> CallbackResult {
    let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);

    // The Caps Lock proxy (F18) release ends the Caps Lock modifier — its quick
    // press/release here is what fires the tap action (e.g. Esc). Drop the event.
    if keycode == crate::capsmap::F18_KEYCODE && CAPS_PROXY_ACTIVE.load(Ordering::SeqCst) {
        drive_modifier(
            app,
            app_state,
            state,
            &ModifierEvent {
                key: ModifierKey::CapsLock,
                side: KeySide::Either,
                is_down: false,
            },
            now,
        );
        return CallbackResult::Drop;
    }

    stamp_held_modifiers(state, event);
    CallbackResult::Keep
}

/// Stamp onto a keystroke the modifier flags contributed by keys held in a
/// special role: the hyper combo (⌃⌥⇧⌘) while a hyper key is held, and a
/// remapped key's target modifier while it is held. The latter makes a chord
/// through a remapped key behave as that modifier (so holding Caps Lock→Control
/// and pressing C yields Ctrl+C) even where the OS does not carry the rewritten
/// `flagsChanged` flag forward onto following keystrokes. The OS does not carry
/// it for Caps Lock, which as a lock key leaves it with no held-modifier state.
fn stamp_held_modifiers(state: &Arc<Mutex<TapState>>, event: &CGEvent) {
    let (hyper_active, (remove, add)) = {
        let ts = state.lock_safe();
        (ts.hyper_active, ts.remap_stamp.clone())
    };
    if !hyper_active && remove.is_empty() && add.is_empty() {
        return;
    }
    let mut flags = event.get_flags();
    // Replace held remapped keys' source modifiers with their targets, so a
    // chord through them carries only the target (a remapped momentary modifier
    // must not leave both set, e.g. Control→Control+Command). Remove first,
    // then add, so a target that coincides with a removed source still lands.
    for modifier in &remove {
        flags.remove(keycodes::flag_for(*modifier));
    }
    for modifier in &add {
        flags.insert(keycodes::flag_for(*modifier));
    }
    // Hyper forces the full ⌃⌥⇧⌘ combo, so apply it last — it must win over a
    // remap that removed one of those flags as its source.
    if hyper_active {
        for modifier in HYPER_MODIFIERS {
            flags.insert(keycodes::flag_for(modifier));
        }
    }
    event.set_flags(flags);
}

/// Run an action on the main thread (UI and webview calls require it).
fn dispatch_async(app: &AppHandle, action: AppAction) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(state) = handle.try_state::<AppState>()
            && let Err(e) = crate::actions::dispatch(&action, &handle, state.inner())
        {
            tracing::warn!(error = %e, "event-tap action failed");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keycode of a managed modifier on the given side (mirrors the tap's own
    /// `modifier_for_keycode` table).
    fn keycode(modkey: ModifierKey, side: KeySide) -> i64 {
        match (modkey, side) {
            (ModifierKey::Command, KeySide::Left) => 55,
            (ModifierKey::Command, KeySide::Right) => 54,
            (ModifierKey::Shift, KeySide::Left) => 56,
            (ModifierKey::Shift, KeySide::Right) => 60,
            (ModifierKey::Control, KeySide::Left) => 59,
            (ModifierKey::Control, KeySide::Right) => 62,
            (ModifierKey::Option, KeySide::Left) => 58,
            (ModifierKey::Option, KeySide::Right) => 61,
            (ModifierKey::CapsLock, _) => 57,
            (ModifierKey::Function, _) => 63,
            other => panic!("unmapped test keycode for {other:?}"),
        }
    }

    #[test]
    fn left_right_modifiers_read_their_device_bit() {
        // Left Command's down/up follows NX_DEVICELCMDKEYMASK in the flags,
        // never the (sideless) generic Command bit.
        let lcmd = keycode(ModifierKey::Command, KeySide::Left);
        let bit = keycodes::device_flag_for_keycode(lcmd).unwrap();
        let mut caps = false;

        assert!(derive_is_down(lcmd, ModifierKey::Command, bit, &mut caps));
        assert!(!derive_is_down(lcmd, ModifierKey::Command, 0, &mut caps));
        // The generic Command flag alone, without the device bit, is a release.
        let generic = keycodes::flag_for(ModifierKey::Command).bits();
        assert!(!derive_is_down(
            lcmd,
            ModifierKey::Command,
            generic,
            &mut caps
        ));
    }

    #[test]
    fn left_and_right_of_a_modifier_are_tracked_independently() {
        let lshift = keycode(ModifierKey::Shift, KeySide::Left);
        let rshift = keycode(ModifierKey::Shift, KeySide::Right);
        let lbit = keycodes::device_flag_for_keycode(lshift).unwrap();
        let rbit = keycodes::device_flag_for_keycode(rshift).unwrap();
        let mut caps = false;

        // Holding only the left side: left reads down, right reads up.
        assert!(derive_is_down(lshift, ModifierKey::Shift, lbit, &mut caps));
        assert!(!derive_is_down(rshift, ModifierKey::Shift, lbit, &mut caps));
        // Both sides held: each reads its own bit.
        assert!(derive_is_down(
            rshift,
            ModifierKey::Shift,
            lbit | rbit,
            &mut caps
        ));
    }

    #[test]
    fn device_bit_keys_ignore_remembered_state_after_a_restart() {
        // Regression guard: the old toggle inverted when a tap restart swallowed
        // a transition mid-hold. Reading the flags makes a held key read `down`
        // on every event regardless of accumulated parity.
        let lctrl = keycode(ModifierKey::Control, KeySide::Left);
        let bit = keycodes::device_flag_for_keycode(lctrl).unwrap();
        let mut caps = false;
        for _ in 0..5 {
            assert!(
                derive_is_down(lctrl, ModifierKey::Control, bit, &mut caps),
                "a still-held key must keep reading as down"
            );
        }
    }

    #[test]
    fn fn_key_reads_its_generic_flag_bit() {
        let fn_code = keycode(ModifierKey::Function, KeySide::Either);
        assert!(keycodes::device_flag_for_keycode(fn_code).is_none());
        let bit = keycodes::flag_for(ModifierKey::Function).bits();
        let mut caps = false;

        assert!(derive_is_down(
            fn_code,
            ModifierKey::Function,
            bit,
            &mut caps
        ));
        assert!(!derive_is_down(
            fn_code,
            ModifierKey::Function,
            0,
            &mut caps
        ));
    }

    #[test]
    fn caps_lock_toggles_because_its_flag_tracks_lock_not_key() {
        let caps_code = keycode(ModifierKey::CapsLock, KeySide::Either);
        assert!(keycodes::device_flag_for_keycode(caps_code).is_none());
        let mut caps = false;

        // The AlphaShift bit reflects lock state, so down/up alternates on each
        // event regardless of the flags carried.
        let alpha = keycodes::flag_for(ModifierKey::CapsLock).bits();
        assert!(derive_is_down(
            caps_code,
            ModifierKey::CapsLock,
            alpha,
            &mut caps
        ));
        assert!(!derive_is_down(
            caps_code,
            ModifierKey::CapsLock,
            0,
            &mut caps
        ));
        assert!(derive_is_down(
            caps_code,
            ModifierKey::CapsLock,
            alpha,
            &mut caps
        ));
    }
}
