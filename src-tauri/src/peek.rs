//! Quick Peek: summon an application while its trigger is held, then put
//! things back when it is released.
//!
//! Two hold-capable paths drive this:
//!
//! * a global hotkey whose action is `LaunchApp` with `quick_peek` — the
//!   shortcut handler in `main` calls [`begin`] on press and [`end`] on
//!   release;
//! * a modifier rule with the same action — the event tap does likewise on
//!   the modifier's down/up transitions.
//!
//! Paths with no release moment (the tray menu, the UI's run button) call
//! [`toggle`] instead. [`cancel`] dismisses unconditionally
//! and is called wherever a release event could be lost (hotkey
//! re-registration, event-tap restart), so a peek can never get stuck.
//!
//! Transitions arrive from several threads (the event tap, the shortcut
//! handler), so applying their side effects one-by-one could interleave out
//! of order or drop the cleanup of an already-replaced summon. Instead, a
//! transition only updates the pure [`PeekState`] machine — including a debt
//! list of summoned apps awaiting hiding — and schedules [`reconcile`] on the
//! main thread, which drains whatever the *latest* state says is owed. The
//! reconciler is idempotent, so no matter how transitions race, the last run
//! converges the screen onto the current state. An app whose launch is still
//! in flight cannot be hidden yet; its debt is requeued and retried briefly
//! so a quick press-and-release does not pop the app up after its dismissal.

use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_app_kit::{NSRunningApplication, NSWorkspace, NSWorkspaceOpenConfiguration};
use objc2_foundation::{NSError, NSString};
use tauri::AppHandle;

/// How often hide passes run, and the overall pass budget per debt (~3 s):
/// long enough for the launch a quick press fired to come up, plus the
/// confirmation watch below.
const HIDE_RETRY_INTERVAL_MS: u64 = 100;
const HIDE_PASS_LIMIT: u8 = 30;
/// Once a hide has landed and the app reports `isFinishedLaunching`, how many
/// further passes watch it: a cold launch reports finished *before*
/// LaunchServices applies its activation, so an activation arriving just
/// after the hide would re-front the app with the debt already settled.
/// An app that never posts its launch notification keeps `isFinishedLaunching`
/// false forever; it is watched (and re-hidden if it re-fronts) for the
/// remainder of the pass budget instead.
const HIDE_CONFIRM_PASSES: u8 = 3;
/// How many passes a hide debt may wait on an unresolved open before giving
/// up (~30 s at [`HIDE_RETRY_INTERVAL_MS`]). The open worker abandons a wedged
/// request after 10 s but does not clear the unresolved mark (only the
/// completion handler does), so a handler that never fires would otherwise
/// keep the debt — and its 100 ms retry thread — requeueing forever.
const OPEN_UNRESOLVED_PASS_LIMIT: u16 = 300;

/// Which input source started a peek. A release only dismisses the peek its
/// own press started, so e.g. releasing hotkey A does not tear down a peek
/// that modifier B has since taken over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// A global hotkey, identified by the shortcut's id.
    Hotkey(u32),
    /// A modifier rule, identified by the physical keycode.
    Modifier(i64),
    /// A path with no release moment (tray, leader, UI).
    Toggle,
}

/// The single peek machine, shared by the hotkey handler and the event tap.
/// Sharing is what keeps a second trigger from double-summoning.
static STATE: Mutex<PeekState> = Mutex::new(PeekState::new());

/// Summon `bundle_id`, remembering the frontmost app it covers so the
/// matching [`end`] can restore it. A no-op when this app is already being
/// peeked (the same trigger repeating must not summon twice; a second trigger
/// takes over ownership instead) or already frontmost (nothing to peek — and
/// dismissing would hide the app the user was really using).
pub fn begin(app: &AppHandle, trigger: Trigger, bundle_id: &str) {
    let frontmost = crate::frontmost::current_bundle_id();
    let changed = STATE
        .lock()
        .unwrap()
        .begin(trigger, bundle_id, frontmost.as_deref());
    if changed {
        schedule(app);
    }
}

/// Dismiss the peek `trigger` owns, if any: hide the summoned app and bring
/// back the one it covered. Safe to call on every release — a trigger that
/// owns nothing is a no-op.
pub fn end(app: &AppHandle, trigger: Trigger) {
    if STATE.lock().unwrap().end(trigger) {
        schedule(app);
    }
}

/// Dismiss any active peek regardless of its trigger. Called where a release
/// event could be lost, e.g. before hotkeys are re-registered or the event
/// tap is restarted.
pub fn cancel(app: &AppHandle) {
    if STATE.lock().unwrap().dismiss() {
        schedule(app);
    }
}

/// Begin if idle, dismiss if this target is already peeked — for trigger
/// paths that have no key-release moment.
pub fn toggle(app: &AppHandle, bundle_id: &str) {
    let frontmost = crate::frontmost::current_bundle_id();
    let changed = {
        let mut state = STATE.lock().unwrap();
        if state.is_peeking(bundle_id) {
            state.dismiss()
        } else {
            state.begin(Trigger::Toggle, bundle_id, frontmost.as_deref())
        }
    };
    if changed {
        schedule(app);
    }
}

/// What is currently summoned, which trigger holds it, which app it covers,
/// and what the screen is still owed (hides and a restore). Pure, so the
/// transition logic is unit-testable without AppKit.
#[derive(Debug)]
struct PeekState {
    active: Option<ActivePeek>,
    /// Summoned apps awaiting hiding (an app whose launch is in flight cannot
    /// hide yet) or whose landed hide is still being confirmed.
    pending_hide: Vec<HideDebt>,
    /// The covered app to re-activate after a dismissal.
    pending_restore: Option<String>,
}

/// One summoned app the screen still owes a hide.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HideDebt {
    /// Bundle id of the app to hide.
    bundle: String,
    /// Passes spent on this debt so far, across both stages — the overall
    /// give-up bound.
    passes: u8,
    /// Confirmation passes that observed the hide holding since it first
    /// landed with the launch finished; `None` until then.
    confirmed: Option<u8>,
    /// Earliest moment ([`now_ms`] clock) the next pass may process this
    /// debt. Transitions reconcile immediately, so without a due time their
    /// passes would advance the confirmation watch ahead of schedule.
    due_at: u64,
    /// Passes spent waiting for the app's open request to resolve. Tracked
    /// separately from `passes` (the open's budget is longer than the hide's)
    /// so a wedged open is eventually abandoned instead of requeued forever.
    unresolved_passes: u16,
}

impl HideDebt {
    fn new(bundle: String) -> Self {
        Self {
            bundle,
            passes: 0,
            confirmed: None,
            due_at: 0,
            unresolved_passes: 0,
        }
    }
}

#[derive(Debug)]
struct ActivePeek {
    /// The trigger whose release dismisses this peek.
    trigger: Trigger,
    /// Bundle id of the summoned app.
    target: String,
    /// Bundle id of the app the unwind should land on. `None` when unknown —
    /// nothing to restore then.
    previous: Option<String>,
    /// Whether the reconciler has already issued this peek's summon.
    summoned: bool,
    /// Whether the unwind owes a restore even if this peek never summons —
    /// true when it absorbed the debt of a chain that already disturbed the
    /// screen (an unsettled dismissal or a summoned peek it replaced).
    restore_owed: bool,
}

/// One reconciler pass's worth of OS work, drained from the state.
#[derive(Debug, Default, PartialEq, Eq)]
struct Work {
    hide: Vec<HideDebt>,
    restore: Option<String>,
    summon: Option<String>,
}

impl PeekState {
    const fn new() -> Self {
        Self {
            active: None,
            pending_hide: Vec::new(),
            pending_restore: None,
        }
    }

    fn is_peeking(&self, target: &str) -> bool {
        self.active.as_ref().is_some_and(|a| a.target == target)
    }

    /// Returns whether the screen needs reconciling.
    fn begin(&mut self, trigger: Trigger, target: &str, frontmost: Option<&str>) -> bool {
        if let Some(active) = self.active.as_mut() {
            if active.target == target {
                // Already summoned: the newest trigger takes over, so its
                // release is the one that dismisses.
                active.trigger = trigger;
                return false;
            }
            let old = self.active.take().expect("checked above");
            if old.summoned {
                self.pending_hide.push(HideDebt::new(old.target));
            }
            if frontmost == Some(target) {
                // The new target is already frontmost (the user switched to
                // it while peeking): retire the old peek but do not adopt the
                // app the user is now working in — a later release would hide
                // it. No restore either; the user is already where they want
                // to be.
                return old.summoned;
            }
            // Replacing keeps the old peek's `previous`, so the eventual
            // dismissal restores the app the user actually started from — not
            // the intermediate peeked one.
            self.active = Some(ActivePeek {
                trigger,
                target: target.to_owned(),
                previous: old.previous,
                summoned: false,
                restore_owed: old.restore_owed || old.summoned,
            });
            return true;
        }
        if let Some(unsettled) = self.pending_restore.take() {
            // A dismissal has not been reconciled yet, so the screen may
            // still show the old peek and `frontmost` is not where the user
            // really was. Absorb the restore debt as this peek's `previous`
            // instead of restoring mid-peek — even when it names this very
            // target, so the unwind still knows where the user belongs.
            self.active = Some(ActivePeek {
                trigger,
                target: target.to_owned(),
                previous: Some(unsettled),
                summoned: false,
                restore_owed: true,
            });
            return true;
        }
        if frontmost == Some(target) {
            // Already frontmost: there is nothing to peek, and a later
            // dismissal would hide the app the user was working in.
            return false;
        }
        self.active = Some(ActivePeek {
            trigger,
            target: target.to_owned(),
            previous: frontmost.map(str::to_owned),
            summoned: false,
            restore_owed: false,
        });
        true
    }

    fn end(&mut self, trigger: Trigger) -> bool {
        if self
            .active
            .as_ref()
            .is_none_or(|active| active.trigger != trigger)
        {
            // A stale release (its peek was replaced or taken over) must not
            // tear down the current holder's peek.
            return false;
        }
        self.dismiss()
    }

    fn dismiss(&mut self) -> bool {
        let Some(active) = self.active.take() else {
            return false;
        };
        if active.summoned {
            if active.previous.as_deref() == Some(active.target.as_str()) {
                // The unwind lands on the peeked app itself (it was the
                // unsettled restore target): never hide it, just make sure it
                // ends up frontmost again — a harmless no-op when the user is
                // still on it, a real restore when they switched away
                // mid-peek.
                self.pending_restore = active.previous;
            } else {
                self.pending_hide.push(HideDebt::new(active.target));
                self.pending_restore = active.previous;
            }
        } else if active.restore_owed {
            // This peek never touched the screen itself, but it absorbed an
            // unsettled restore — hand that debt back.
            self.pending_restore = active.previous;
        }
        true
    }

    /// Drain the work the screen is owed *right now*. Hide debt for the
    /// currently active target is discarded — re-summoning it cancelled that
    /// debt, and hiding it would fight the peek in progress. Debts not yet
    /// due stay queued for a later pass.
    fn take_work(&mut self, now: u64) -> Work {
        let active_target = self.active.as_ref().map(|a| a.target.clone());
        self.pending_hide
            .retain(|debt| Some(&debt.bundle) != active_target.as_ref());
        let (hide, later) = std::mem::take(&mut self.pending_hide)
            .into_iter()
            .partition(|debt| debt.due_at <= now);
        self.pending_hide = later;
        let summon = match self.active.as_mut() {
            Some(active) if !active.summoned => {
                active.summoned = true;
                Some(active.target.clone())
            }
            _ => None,
        };
        Work {
            hide,
            restore: self.pending_restore.take(),
            summon,
        }
    }

    /// Put a hide that is not settled yet (launch in flight, or its landing
    /// still being confirmed) back on the debt list for a later pass.
    fn requeue_hide(&mut self, debt: HideDebt) {
        self.pending_hide.push(debt);
    }

    /// When the earliest queued debt falls due, if any.
    fn next_due(&self) -> Option<u64> {
        self.pending_hide.iter().map(|debt| debt.due_at).min()
    }
}

/// Queue a reconciler pass on the main thread (AppKit calls belong there).
fn schedule(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || reconcile(&handle));
}

/// Converge the screen onto the current state: settle hide debts, restore the
/// covered app, summon the active target. Runs only on the main thread, so
/// passes never interleave; being drained from the latest state makes each
/// pass idempotent and the last one authoritative, however transitions raced.
fn reconcile(app: &AppHandle) {
    let now = now_ms();
    let work = STATE.lock().unwrap().take_work(now);

    for debt in work.hide {
        if let Some(next) = try_hide(debt, now) {
            STATE.lock().unwrap().requeue_hide(next);
        }
    }
    if let Some(previous) = work.restore {
        restore(&previous);
    }
    if let Some(target) = work.summon {
        summon(&target);
    }

    // Arm a timer for the earliest queued debt. Timers may overlap with
    // passes that transitions trigger; the per-debt due times make extra
    // passes harmless, so no coalescing is needed for correctness.
    if let Some(due) = STATE.lock().unwrap().next_due() {
        retry_at(app, due.saturating_sub(now));
    }
}

/// Milliseconds on a process-local monotonic clock, for debt due times.
fn now_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Schedule another reconciler pass after `delay_ms`.
fn retry_at(app: &AppHandle, delay_ms: u64) {
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        schedule(&handle);
    });
}

/// Bring `bundle_id` to the front, launching it first if it is not running.
///
/// Activation goes through a LaunchServices open request rather than
/// `NSRunningApplication`: an activation request from an app that is not
/// itself frontmost is accepted (the call returns `true`) but silently
/// dropped by macOS's cooperative-activation policy, so the summoned app
/// would never actually come forward. LaunchServices activation is honored,
/// and also covers the not-yet-running case.
///
/// Requests run on a dedicated worker, one at a time, each awaited via its
/// completion handler: FIFO order keeps a later restore from overtaking an
/// earlier summon, and the handler surfaces failed requests. Staying off the
/// main thread keeps a slow LaunchServices from stalling the UI; a hide
/// racing an in-flight open is absorbed by the hide debt's confirmation
/// watch, whose budget does not start until the open resolves (see
/// [`open_unresolved`]).
///
/// Known platform caveat: while one of our own Carbon-registered hotkeys is
/// held, the system may defer or drop the activation (the request resolves
/// without error but the app does not front until the key is released). The
/// event-tap-driven paths (modifier rules, leader sequences) are unaffected.
fn summon(bundle_id: &str) {
    static OPENS: OnceLock<Sender<String>> = OnceLock::new();
    let sender = OPENS.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<String>();
        let _ = std::thread::Builder::new()
            .name("tomari-peek-open".into())
            .spawn(move || {
                for bundle in rx {
                    open_and_wait(&bundle);
                }
            });
        tx
    });
    mark_open_queued(bundle_id);
    if sender.send(bundle_id.to_owned()).is_err() {
        mark_open_resolved(bundle_id);
        tracing::warn!(bundle_id, "quick peek open worker is gone");
    }
}

/// Open requests queued or in flight per bundle id. A hide debt must not
/// spend its budget while an open for its app is still unresolved: the
/// worker serializes requests, so a wedged earlier request could otherwise
/// outlive the debt entirely and the app would pop up with nobody left to
/// hide it. The count is decremented by the request's completion handler —
/// never by the worker's queue-wait timeout, which abandons the queue slot
/// while the request itself stays in flight.
static UNRESOLVED_OPENS: Mutex<Option<std::collections::HashMap<String, u32>>> = Mutex::new(None);

fn mark_open_queued(bundle: &str) {
    let mut guard = UNRESOLVED_OPENS.lock().unwrap();
    *guard
        .get_or_insert_with(Default::default)
        .entry(bundle.to_owned())
        .or_insert(0) += 1;
}

fn mark_open_resolved(bundle: &str) {
    let mut guard = UNRESOLVED_OPENS.lock().unwrap();
    if let Some(map) = guard.as_mut()
        && let Some(count) = map.get_mut(bundle)
    {
        *count = count.saturating_sub(1);
        if *count == 0 {
            map.remove(bundle);
        }
    }
}

fn open_unresolved(bundle: &str) -> bool {
    UNRESOLVED_OPENS
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|map| map.contains_key(bundle))
}

/// Ask LaunchServices to open (launch or activate) `bundle` and wait for the
/// request to resolve, with a generous timeout so a wedged LaunchServices
/// cannot stall the open queue forever. The timeout only abandons the queue
/// slot: the request is not cancelled, so the unresolved mark is cleared by
/// the completion handler whenever it eventually fires, keeping the app's
/// hide debt parked until the late activation can no longer slip past it.
fn open_and_wait(bundle: &str) {
    let started = Instant::now();
    let workspace = NSWorkspace::sharedWorkspace();
    let Some(url) = workspace.URLForApplicationWithBundleIdentifier(&NSString::from_str(bundle))
    else {
        tracing::warn!(bundle_id = %bundle, "quick peek: no application for bundle id");
        mark_open_resolved(bundle);
        return;
    };

    let (done_tx, done_rx) = mpsc::channel::<Option<String>>();
    let owner = bundle.to_owned();
    let handler = RcBlock::new(
        move |_app: *mut NSRunningApplication, error: *mut NSError| {
            mark_open_resolved(&owner);
            let failure = unsafe { error.as_ref() }.map(|e| e.localizedDescription().to_string());
            let _ = done_tx.send(failure);
        },
    );
    workspace.openApplicationAtURL_configuration_completionHandler(
        &url,
        &NSWorkspaceOpenConfiguration::configuration(),
        Some(&handler),
    );

    match done_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(None) => {
            tracing::debug!(bundle_id = %bundle, took_ms = started.elapsed().as_millis() as u64, "peek open done");
        }
        Ok(Some(error)) => tracing::warn!(bundle_id = %bundle, %error, "quick peek open failed"),
        Err(_) => {
            tracing::warn!(bundle_id = %bundle, "quick peek open still pending; moving on")
        }
    }
}

/// Re-activate the app a dismissed peek covered. Unlike [`summon`] this is a
/// no-op when the app is not running — restoring must not relaunch an app the
/// user quit mid-peek. Two accepted edge cases: the check is best-effort (the
/// app can still exit between it and the `open`, which then relaunches it),
/// and LaunchServices reopen semantics may create a fresh window when the
/// user closed the app's last window while peeking.
fn restore(bundle_id: &str) {
    if running_app(bundle_id).is_none() {
        return;
    }
    summon(bundle_id);
}

/// Run one pass over a hide debt. Returns the requeued debt when it is not
/// settled yet, `None` once it is (or once the pass budget is spent).
///
/// Each pass ensures the app is hidden (requesting a hide only when it is
/// not), so an already-hidden app is merely observed, never fought over. The
/// debt settles only after the hide has held for [`HIDE_CONFIRM_PASSES`]
/// once the launch finished — closing the window where a cold launch's
/// pending activation re-fronts the app right after a hide. The confirmation
/// count never advances on a pass that could not (re-)establish the hide.
fn try_hide(debt: HideDebt, now: u64) -> Option<HideDebt> {
    if open_unresolved(&debt.bundle) {
        // The open that summoned this app has not resolved yet (it may still
        // be queued behind a slow request) — hold the debt without spending
        // the hide budget or advancing the watch, or it could lapse before
        // the app even appears. Give up once the unresolved wait runs long:
        // a completion handler that never fires would otherwise requeue this
        // debt every interval forever.
        if debt.unresolved_passes >= OPEN_UNRESOLVED_PASS_LIMIT {
            tracing::warn!(
                bundle_id = %debt.bundle,
                "quick peek: open never resolved; abandoning hide"
            );
            // Release the stuck mark too: leaving it set would make every
            // future Quick Peek of this bundle wait out the full timeout and
            // be abandoned again (its hide could never settle).
            mark_open_resolved(&debt.bundle);
            return None;
        }
        return Some(HideDebt {
            due_at: now + HIDE_RETRY_INTERVAL_MS,
            unresolved_passes: debt.unresolved_passes + 1,
            ..debt
        });
    }
    if debt.passes >= HIDE_PASS_LIMIT {
        return None;
    }
    let next = |confirmed| {
        Some(HideDebt {
            bundle: debt.bundle.clone(),
            passes: debt.passes + 1,
            confirmed,
            due_at: now + HIDE_RETRY_INTERVAL_MS,
            unresolved_passes: debt.unresolved_passes,
        })
    };

    let Some(app) = running_app(&debt.bundle) else {
        // Not running: once the hide has landed, a vanished app settles the
        // debt; before that the launch may still be in flight, so retry.
        return if debt.confirmed.is_some() {
            None
        } else {
            next(None)
        };
    };

    let hidden = app.isHidden() || app.hide();
    if !hidden {
        // Could not (re-)establish the hide: spend budget without advancing
        // the confirmation count.
        return next(debt.confirmed);
    }

    match debt.confirmed {
        // The hide has landed and the launch is done — start (or advance)
        // the confirmation watch.
        None if app.isFinishedLaunching() => next(Some(0)),
        // Hidden, but the launch has not reported finishing; keep watching
        // within the budget (some apps never post the notification).
        None => next(None),
        Some(c) if c + 1 >= HIDE_CONFIRM_PASSES => None,
        Some(c) => next(Some(c + 1)),
    }
}

fn running_app(bundle_id: &str) -> Option<Retained<NSRunningApplication>> {
    NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(bundle_id))
        .firstObject()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOTKEY: Trigger = Trigger::Hotkey(1);
    const MODIFIER: Trigger = Trigger::Modifier(54);

    const FINDER: &str = "com.apple.finder";
    const SAFARI: &str = "com.apple.Safari";
    const TERMINAL: &str = "com.apple.Terminal";

    fn debt(bundle: &str, passes: u8) -> HideDebt {
        HideDebt {
            bundle: bundle.to_owned(),
            passes,
            confirmed: None,
            due_at: 0,
            unresolved_passes: 0,
        }
    }

    fn work(hide: &[(&str, u8)], restore: Option<&str>, summon: Option<&str>) -> Work {
        Work {
            hide: hide.iter().map(|(b, n)| debt(b, *n)).collect(),
            restore: restore.map(str::to_owned),
            summon: summon.map(str::to_owned),
        }
    }

    #[test]
    fn begin_summons_and_end_hides_and_restores() {
        let mut s = PeekState::new();
        assert!(s.begin(HOTKEY, FINDER, Some(SAFARI)));
        assert_eq!(s.take_work(0), work(&[], None, Some(FINDER)));
        assert!(s.end(HOTKEY));
        assert_eq!(s.take_work(0), work(&[(FINDER, 0)], Some(SAFARI), None));
    }

    #[test]
    fn repeated_begin_for_the_same_target_is_ignored() {
        let mut s = PeekState::new();
        s.begin(HOTKEY, FINDER, Some(SAFARI));
        s.take_work(0);
        // A key repeat fires press again — no new work, and `previous` must
        // not become the now-frontmost target.
        assert!(!s.begin(HOTKEY, FINDER, Some(FINDER)));
        s.end(HOTKEY);
        assert_eq!(s.take_work(0), work(&[(FINDER, 0)], Some(SAFARI), None));
    }

    #[test]
    fn a_second_trigger_takes_over_the_same_target() {
        let mut s = PeekState::new();
        s.begin(HOTKEY, FINDER, Some(SAFARI));
        s.take_work(0);
        assert!(!s.begin(MODIFIER, FINDER, Some(FINDER)));
        // The first trigger's release no longer owns the peek.
        assert!(!s.end(HOTKEY));
        assert!(s.is_peeking(FINDER));
        assert!(s.end(MODIFIER));
        assert_eq!(s.take_work(0), work(&[(FINDER, 0)], Some(SAFARI), None));
    }

    #[test]
    fn a_fast_tap_before_any_reconcile_leaves_the_screen_untouched() {
        let mut s = PeekState::new();
        // Press and release land before the reconciler ever ran: the summon
        // was never issued, so the unwind owes no hide and no restore (which
        // would steal the frontmost app or hide one the user launched).
        assert!(s.begin(HOTKEY, FINDER, Some(SAFARI)));
        assert!(s.end(HOTKEY));
        assert_eq!(s.take_work(0), Work::default());
    }

    #[test]
    fn end_without_an_active_peek_is_a_noop() {
        assert!(!PeekState::new().end(HOTKEY));
    }

    #[test]
    fn peeking_the_frontmost_app_is_a_noop() {
        let mut s = PeekState::new();
        // Summoning would change nothing, and the dismissal would hide the
        // app the user was actually working in.
        assert!(!s.begin(HOTKEY, FINDER, Some(FINDER)));
        assert!(!s.end(HOTKEY));
        assert_eq!(s.take_work(0), Work::default());
    }

    #[test]
    fn replacing_a_peek_hides_it_and_keeps_the_original_previous() {
        let mut s = PeekState::new();
        s.begin(HOTKEY, FINDER, Some(SAFARI));
        s.take_work(0);
        assert!(s.begin(MODIFIER, TERMINAL, Some(FINDER)));
        assert_eq!(s.take_work(0), work(&[(FINDER, 0)], None, Some(TERMINAL)));
        assert!(s.end(MODIFIER));
        assert_eq!(s.take_work(0), work(&[(TERMINAL, 0)], Some(SAFARI), None));
    }

    #[test]
    fn replacing_with_an_already_frontmost_target_only_retires_the_old_peek() {
        let mut s = PeekState::new();
        s.begin(HOTKEY, FINDER, Some(SAFARI));
        s.take_work(0);
        // The user switched to Terminal while peeking Finder, then pressed
        // Terminal's peek trigger: hide Finder, but do not adopt Terminal —
        // the release would hide the app they are working in.
        assert!(s.begin(MODIFIER, TERMINAL, Some(TERMINAL)));
        assert_eq!(s.take_work(0), work(&[(FINDER, 0)], None, None));
        assert!(!s.end(MODIFIER));
    }

    #[test]
    fn a_stale_release_does_not_end_the_replacing_peek() {
        let mut s = PeekState::new();
        s.begin(HOTKEY, FINDER, Some(SAFARI));
        s.begin(MODIFIER, TERMINAL, Some(FINDER));
        // The hotkey's peek was replaced; its release must not dismiss the
        // modifier's peek.
        assert!(!s.end(HOTKEY));
        assert!(s.is_peeking(TERMINAL));
    }

    #[test]
    fn dismiss_ends_any_peek_regardless_of_trigger() {
        let mut s = PeekState::new();
        s.begin(HOTKEY, FINDER, Some(SAFARI));
        s.take_work(0);
        assert!(s.dismiss());
        assert!(!s.dismiss());
        assert_eq!(s.take_work(0), work(&[(FINDER, 0)], Some(SAFARI), None));
    }

    #[test]
    fn only_summoned_peeks_owe_a_hide_across_replacements() {
        let mut s = PeekState::new();
        s.begin(HOTKEY, FINDER, Some(SAFARI));
        s.take_work(0);
        // Two rapid replacements before the next reconciler pass: the
        // summoned Finder owes a hide, the never-summoned Terminal does not
        // (hiding it could catch a copy the user launched themselves).
        s.begin(MODIFIER, TERMINAL, Some(FINDER));
        s.begin(HOTKEY, "com.apple.Notes", Some(TERMINAL));
        let w = s.take_work(0);
        assert_eq!(w.hide, vec![debt(FINDER, 0)]);
        assert_eq!(w.summon.as_deref(), Some("com.apple.Notes"));
    }

    #[test]
    fn begin_after_an_unsettled_dismiss_inherits_the_restore() {
        let mut s = PeekState::new();
        s.begin(HOTKEY, FINDER, Some(SAFARI));
        s.take_work(0);
        s.end(HOTKEY);
        // The dismissal has not been reconciled, so Finder is still showing
        // and frontmost. The next peek must not record it as `previous`, nor
        // let the pending restore fire mid-peek — Safari is restored only
        // when the new peek unwinds.
        assert!(s.begin(MODIFIER, TERMINAL, Some(FINDER)));
        assert_eq!(s.take_work(0), work(&[(FINDER, 0)], None, Some(TERMINAL)));
        assert!(s.end(MODIFIER));
        assert_eq!(s.take_work(0), work(&[(TERMINAL, 0)], Some(SAFARI), None));
    }

    #[test]
    fn peeking_the_unsettled_restore_target_lands_back_on_it() {
        let mut s = PeekState::new();
        // Safari → peek Finder → release → (before reconcile) peek Safari.
        s.begin(HOTKEY, FINDER, Some(SAFARI));
        s.take_work(0);
        s.end(HOTKEY);
        assert!(s.begin(MODIFIER, SAFARI, Some(FINDER)));
        assert_eq!(s.take_work(0), work(&[(FINDER, 0)], None, Some(SAFARI)));
        // Unwinding lands on Safari itself: it must not be hidden, only
        // re-activated — a no-op when the user is still on it, and the way
        // back when they switched to another app mid-peek.
        assert!(s.end(MODIFIER));
        assert_eq!(s.take_work(0), work(&[], Some(SAFARI), None));
    }

    #[test]
    fn an_unsummoned_replacement_still_restores_on_unwind() {
        let mut s = PeekState::new();
        s.begin(HOTKEY, FINDER, Some(SAFARI));
        s.take_work(0);
        // Terminal replaces Finder but is dismissed before its summon was
        // ever issued: the screen still shows Finder, so the unwind must
        // hide it and restore Safari.
        s.begin(MODIFIER, TERMINAL, Some(FINDER));
        assert!(s.end(MODIFIER));
        assert_eq!(s.take_work(0), work(&[(FINDER, 0)], Some(SAFARI), None));
    }

    #[test]
    fn requeued_hide_comes_back_unchanged() {
        let mut s = PeekState::new();
        s.begin(HOTKEY, FINDER, Some(SAFARI));
        s.take_work(0);
        s.dismiss();
        let w = s.take_work(0);
        assert_eq!(w.hide, vec![debt(FINDER, 0)]);
        s.requeue_hide(debt(FINDER, 1));
        assert_eq!(s.take_work(0).hide, vec![debt(FINDER, 1)]);
        // A debt in its confirmation watch survives the round trip too.
        s.requeue_hide(HideDebt {
            confirmed: Some(1),
            ..debt(FINDER, 2)
        });
        assert_eq!(
            s.take_work(0).hide,
            vec![HideDebt {
                confirmed: Some(1),
                ..debt(FINDER, 2)
            }]
        );
    }

    #[test]
    fn a_debt_is_not_drained_before_it_falls_due() {
        let mut s = PeekState::new();
        s.requeue_hide(HideDebt {
            due_at: 100,
            ..debt(FINDER, 1)
        });
        // A pass triggered by an unrelated transition at t=90 must leave the
        // debt queued, or the confirmation watch would advance early.
        assert!(s.take_work(90).hide.is_empty());
        assert_eq!(s.next_due(), Some(100));
        assert_eq!(
            s.take_work(100).hide,
            vec![HideDebt {
                due_at: 100,
                ..debt(FINDER, 1)
            }]
        );
        assert_eq!(s.next_due(), None);
    }

    #[test]
    fn re_summoning_a_target_cancels_its_pending_hide() {
        let mut s = PeekState::new();
        s.begin(HOTKEY, FINDER, Some(SAFARI));
        s.take_work(0);
        s.dismiss();
        // Peeked again before the reconciler settled the hide debt: hiding
        // now would fight the new peek.
        s.begin(HOTKEY, FINDER, Some(SAFARI));
        let w = s.take_work(0);
        assert!(w.hide.is_empty());
        assert_eq!(w.summon.as_deref(), Some(FINDER));
    }

    #[test]
    fn an_unresolved_open_is_abandoned_after_the_pass_limit() {
        // An open whose completion handler never fires keeps the bundle marked
        // unresolved. `try_hide` must eventually give up rather than requeue
        // the debt (and spawn a retry thread) every interval forever.
        mark_open_queued(FINDER);
        let mut current = Some(debt(FINDER, 0));
        let mut requeues = 0u32;
        while let Some(d) = current.take() {
            current = try_hide(d, 0);
            if current.is_some() {
                requeues += 1;
                // Safety net so a regression cannot hang the test run.
                if requeues > OPEN_UNRESOLVED_PASS_LIMIT as u32 + 1 {
                    break;
                }
            }
        }
        // Clear the global mark before asserting so a failure cannot leak it.
        mark_open_resolved(FINDER);
        assert_eq!(requeues, OPEN_UNRESOLVED_PASS_LIMIT as u32);
    }
}
