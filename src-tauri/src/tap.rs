//! Shared scaffolding for the three dedicated-thread `CGEventTap`s
//! ([`crate::eventtap`], [`crate::drag_to_snap`], [`crate::drag_to_move`]):
//! spawn a thread, create the tap, attach its run-loop source, and hand the
//! run loop back to the caller so it can be stopped and joined on teardown.
//!
//! Each tap differs only in its thread name (for logging/`Thread.name`), its
//! [`CGEventTapOptions`] (listen-only vs. active), the event types it
//! subscribes to, and the callback itself — the callback holds all
//! tap-specific decision logic (the modifier engine, the drag state
//! machines, …) and is completely untouched by this module.
//!
//! ## The `CFRunLoopStop` race this closes
//!
//! The thread spawned here must publish its `CFRunLoop` back to the caller so
//! [`RunningTap::drop`] can later call `stop()` on it — but `CFRunLoopStop` is
//! a no-op unless the target run loop is *currently running*
//! (`CFRunLoopRun`/`CFRunLoopRunInMode`). If the run loop handle were sent
//! before entering `CFRunLoopRun()`, a `stop()` that lands in the gap between
//! the send and the `run()` call would be silently dropped, and the run loop
//! would then run forever with nothing left to stop it — `RunningTap::drop`'s
//! `handle.join()` would hang forever.
//!
//! To close that gap, a `CFRunLoopObserver` for `kCFRunLoopEntry` is attached
//! before the loop is entered, and the run loop handle is only published to the
//! caller from *inside* that observer's callback — i.e. only once the run loop
//! has actually started running. Any `stop()` the caller issues after the
//! handshake completes is therefore guaranteed to land on a running loop and
//! take effect immediately, instead of a periodic-wakeup workaround (an
//! `AtomicBool` flag polled through a bounded `CFRunLoopRunInMode` loop), which
//! would burn a wakeup every tick even while fully idle.
//!
//! ## Deadlines, and why a tap can outlive its handle
//!
//! Neither half of a tap's lifecycle is allowed to wait forever. Starting one
//! waits at most [`START_DEADLINE`] for the run loop to report in, and stopping
//! one waits at most [`STOP_DEADLINE`] for its thread to return. Both are
//! reached from paths a person is waiting on — saving settings, waking from
//! sleep, quitting — and a callback stuck in an OS call it cannot cancel must
//! not turn any of those into a hang.
//!
//! Past a deadline the thread is *detached* rather than waited on, which means
//! its `CGEventTap` can still be live when the next one starts. Two taps
//! handling the same input would be worse than one late tap, so every tap
//! carries a liveness flag: [`RunningTap::drop`] clears it before it stops the
//! run loop, and the wrapper around the callback returns early once it is clear.
//! A detached tap therefore still exists but handles nothing further — every
//! event it has not already started on passes through untouched until its thread
//! returns. An event it *had* started on runs to completion and its verdict
//! stands: its side effects are already committed, so discarding only the
//! verdict would hand the app an event whose consequences had happened anyway.
//!
//! The startup handshake takes the same care: the caller's deadline and the
//! thread's hand-over go through one mutex, so a run loop that starts at the
//! very moment the caller gives up is told to stop rather than left running
//! with nobody holding it — and its liveness flag is cleared too, since no
//! `RunningTap` will ever exist to do that later.
use std::os::raw::c_void;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use core_foundation::base::TCFType;
use core_foundation::runloop::{CFRunLoop, CFRunLoopObserver, kCFRunLoopCommonModes};
use core_foundation_sys::mach_port::CFMachPortRef;
use core_foundation_sys::runloop::{
    CFRunLoopActivity, CFRunLoopObserverContext, CFRunLoopObserverCreate, CFRunLoopObserverRef,
    kCFRunLoopEntry,
};
use core_graphics::event::{
    CGEvent, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventTapProxy, CGEventType, CallbackResult,
};

/// The per-event callback a tap runs on its own thread. Boxed so `spawn`'s
/// caller can build it from whatever tap-specific state it closes over
/// (the modifier engine, a drag state machine, …) without that type leaking
/// into this module's signatures.
pub type TapCallback = Box<dyn Fn(CGEventTapProxy, CGEventType, &CGEvent) -> CallbackResult + Send>;

/// How long [`spawn`] waits for a tap's run loop to report that it is running
/// before giving up on it. Creating a tap and entering a run loop is
/// microseconds' work, so this only ever expires on something pathological —
/// but the callers are the settings save, the wake handler and quit, none of
/// which may hang on one.
const START_DEADLINE: Duration = Duration::from_secs(2);

/// How long [`RunningTap::drop`] waits for a tap's thread to return after its
/// run loop is stopped. A callback that is inside an OS call it cannot cancel
/// holds the thread for as long as that call takes; past this the thread is
/// detached and its tap left inert (see the module doc comment) rather than
/// blocking a restart or a quit indefinitely.
const STOP_DEADLINE: Duration = Duration::from_secs(2);

/// How often a bounded join re-checks. Short enough that a normal stop is not
/// perceptibly delayed, long enough not to spin.
const JOIN_POLL: Duration = Duration::from_millis(5);

/// A running event tap: the run loop it is attached to (so it can be stopped),
/// the thread driving it, and the flag that makes its callback inert. Shared by
/// all three taps; dropping it stops the run loop and joins the thread —
/// bounded by [`STOP_DEADLINE`] — invalidating the tap.
pub struct RunningTap {
    label: &'static str,
    run_loop: CFRunLoop,
    thread: Option<JoinHandle<()>>,
    /// Cleared on the way down so a thread that outlives its bounded join
    /// stops handling events. See the module doc comment.
    live: Arc<AtomicBool>,
}

impl Drop for RunningTap {
    fn drop(&mut self) {
        // Retire the tap *before* stopping it. If the thread then outlives the
        // bounded join below, its tap is still live as far as the OS is
        // concerned — this is what keeps it from handling input alongside the
        // tap that replaces it.
        self.live.store(false, Ordering::SeqCst);
        // Stopping the run loop makes `CFRunLoopRun` return; the thread then
        // drops the tap (invalidating it) and exits. Safe to call unconditionally
        // because `spawn` only ever hands out a `RunningTap` whose run loop has
        // already entered `CFRunLoopRun` (see the module doc comment) — so this
        // `stop()` is never a no-op racing against a not-yet-running loop.
        self.run_loop.stop();
        if let Some(handle) = self.thread.take() {
            join_bounded(handle, self.label, STOP_DEADLINE);
        }
    }
}

/// Wait for a tap thread to return, giving up after `deadline`. Returns whether
/// it did.
///
/// `JoinHandle` has no timed join, so this polls `is_finished`. Dropping the
/// handle past the deadline detaches the thread: it runs to completion on its
/// own, and its tap — already retired by [`RunningTap::drop`] — passes events
/// through until it does.
fn join_bounded(handle: JoinHandle<()>, tap_label: &str, deadline: Duration) -> bool {
    let give_up_at = Instant::now() + deadline;
    while !handle.is_finished() {
        if Instant::now() >= give_up_at {
            tracing::warn!(
                tap = tap_label,
                timeout_ms = deadline.as_millis(),
                "tap thread did not stop in time; detaching it (the tap is retired and passes \
                 events through until the thread returns)"
            );
            return false;
        }
        std::thread::sleep(JOIN_POLL);
    }
    let _ = handle.join();
    true
}

/// Re-enable a tap the system disabled (timeout or heavy input), given the mach
/// port published via the `port_holder` passed to the tap's callback factory.
/// A no-op if the port has not been published yet (tap creation still failing
/// or racing startup).
pub fn reenable(port_holder: &AtomicUsize) {
    let port = port_holder.load(Ordering::SeqCst) as CFMachPortRef;
    if !port.is_null() {
        // Safety: `port` is the mach port of a `CGEventTap` created and kept
        // alive by this same tap's thread (published by `spawn` before the
        // callback can observe any events, including the disable notification
        // that leads here); `CGEventTapEnable` is safe to call from any thread.
        unsafe { CGEventTapEnable(port, true) };
    }
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
}

/// (Re)start scaffolding shared by all three taps: spawn a dedicated thread
/// that creates the `CGEventTap`, attaches its run-loop source, and enters
/// `CFRunLoopRun`, then wait for it to signal either a running [`RunningTap`]
/// or a startup failure (e.g. the *Input Monitoring* permission is missing).
///
/// `events` is the list of event types the tap subscribes to. `make_callback`
/// receives the `port_holder` the callback should publish its tap's mach port
/// into (for [`reenable`]) and returns the `Fn` callback itself — all
/// tap-specific state (the modifier engine, drag state machines, …) is
/// captured there, untouched by this helper.
pub fn spawn<F>(
    thread_name: &'static str,
    tap_label: &'static str,
    options: CGEventTapOptions,
    events: Vec<CGEventType>,
    make_callback: F,
) -> Result<RunningTap, String>
where
    F: FnOnce(Arc<AtomicUsize>) -> TapCallback + Send + 'static,
{
    let handshake = Arc::new(Handshake::default());
    let live = Arc::new(AtomicBool::new(true));
    let thread = {
        let handshake = Arc::clone(&handshake);
        let live = Arc::clone(&live);
        std::thread::Builder::new()
            .name(thread_name.into())
            .spawn(move || run_tap(tap_label, options, events, make_callback, &handshake, live))
            .map_err(|e| e.to_string())?
    };

    match handshake.wait(START_DEADLINE) {
        Startup::Running(run_loop) => Ok(RunningTap {
            label: tap_label,
            run_loop,
            thread: Some(thread),
            live,
        }),
        Startup::Failed(e) => {
            // The thread is already returning: it reported the failure instead
            // of entering a run loop.
            live.store(false, Ordering::SeqCst);
            join_bounded(thread, tap_label, STOP_DEADLINE);
            Err(e)
        }
        // The handshake now reads `Abandoned`, so a thread that reaches its run
        // loop after this stops there instead of handling input unheld. Retire
        // the tap as well: no `RunningTap` exists to do it later, so this is the
        // only thing standing between a thread whose `stop()` somehow does not
        // take and a tap handling input that nobody can turn off.
        Startup::Waiting | Startup::Abandoned => {
            live.store(false, Ordering::SeqCst);
            Err(format!(
                "{tap_label} did not start within {} ms",
                START_DEADLINE.as_millis()
            ))
        }
    }
}

/// How far a starting tap thread has got, as the caller sees it.
#[derive(Default)]
enum Startup {
    /// Nothing reported yet.
    #[default]
    Waiting,
    /// The run loop is running and is the caller's to stop.
    Running(CFRunLoop),
    /// The tap could not be created at all (typically no *Input Monitoring*).
    Failed(String),
    /// The caller's deadline expired. A thread that reaches its run loop now
    /// stops at once rather than running with nobody holding it.
    Abandoned,
}

/// The hand-over between [`spawn`] and its tap thread. Both the thread's report
/// and the caller's deadline go through the one mutex, so a run loop that starts
/// at the very moment the caller gives up is either handed over or told to stop
/// — never silently left running.
#[derive(Default)]
struct Handshake {
    state: Mutex<Startup>,
    reported: Condvar,
}

impl Handshake {
    fn lock(&self) -> std::sync::MutexGuard<'_, Startup> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Wait for the thread to report, up to `deadline`. Whatever it reported is
    /// taken, and `Abandoned` is left in its place: the observer only ever reads
    /// that on the give-up path, since it fires at most once.
    fn wait(&self, deadline: Duration) -> Startup {
        let guard = self.lock();
        let (mut guard, _) = self
            .reported
            .wait_timeout_while(guard, deadline, |state| matches!(state, Startup::Waiting))
            .unwrap_or_else(|e| e.into_inner());
        std::mem::replace(&mut *guard, Startup::Abandoned)
    }

    /// Report from the tap thread. Returns whether the caller is still waiting;
    /// `false` means it gave up and this tap must not start.
    fn report(&self, outcome: Startup) -> bool {
        let mut guard = self.lock();
        if matches!(*guard, Startup::Abandoned) {
            return false;
        }
        *guard = outcome;
        self.reported.notify_all();
        true
    }
}

/// Context handed to the `kCFRunLoopEntry` observer callback: where to publish
/// the now-running run loop, and the flag to retire the tap with if nobody is
/// waiting for it any more.
struct EntryContext {
    handshake: Arc<Handshake>,
    live: Arc<AtomicBool>,
}

extern "C" fn on_run_loop_entry(
    _observer: CFRunLoopObserverRef,
    _activity: CFRunLoopActivity,
    info: *mut c_void,
) {
    // Safety: `info` is the `&EntryContext` passed as `context.info` below,
    // kept alive on `run_tap`'s stack for at least as long as the observer is
    // attached to the run loop (removed and dropped only after `CFRunLoopRun`
    // returns). The observer does not repeat, so this fires at most once.
    let ctx = unsafe { &*(info as *const EntryContext) };
    // The loop has just entered `CFRunLoopRun` (or the equivalent internal
    // entry into the current pass), so it is now running — a `stop()` the
    // caller performs after the handshake completes will always land on a live
    // loop rather than racing its entry (see the module doc comment).
    let run_loop = CFRunLoop::get_current();
    if !ctx.handshake.report(Startup::Running(run_loop.clone())) {
        // The caller stopped waiting for us. Nobody holds this tap, so nobody
        // could ever stop it later: retire it here — *before* stopping the loop,
        // and in the same place that learned it was abandoned, so there is no
        // window where a queued source could be handled by a tap with no owner.
        ctx.live.store(false, Ordering::SeqCst);
        run_loop.stop();
    }
}

fn run_tap<F>(
    tap_label: &'static str,
    options: CGEventTapOptions,
    events: Vec<CGEventType>,
    make_callback: F,
    handshake: &Arc<Handshake>,
    live: Arc<AtomicBool>,
) where
    F: FnOnce(Arc<AtomicUsize>) -> TapCallback + Send + 'static,
{
    let port_holder = Arc::new(AtomicUsize::new(0));
    let callback = make_callback(port_holder.clone());

    let callback_live = Arc::clone(&live);
    let tap = match CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        options,
        events,
        move |proxy, etype, event| {
            // Retired: this tap outlived the bounded join of the handle that
            // owned it, and a newer one may already be handling input. Pass
            // everything through untouched until this thread returns.
            if !callback_live.load(Ordering::SeqCst) {
                return CallbackResult::Keep;
            }
            // Deliberately not re-checked after the fact. A callback that got
            // past the line above has already committed its side effects — the
            // engine has taken the modifier, the applier has the gesture, the
            // event may have been rewritten in place — so overriding its verdict
            // to `Keep` would hand the app an event whose consequences already
            // happened, and could leave it a key-down with no key-up. Retirement
            // therefore stops the *next* event, not the one in flight.
            callback(proxy, etype, event)
        },
    ) {
        Ok(tap) => tap,
        Err(()) => {
            handshake.report(Startup::Failed(format!(
                "failed to create {tap_label} — Input Monitoring permission required"
            )));
            return;
        }
    };

    // Publish the mach port so the callback can re-arm the tap if the system
    // disables it after a slow callback or heavy input (see `reenable`).
    port_holder.store(
        tap.mach_port().as_concrete_TypeRef() as usize,
        Ordering::SeqCst,
    );

    let source = match tap.mach_port().create_runloop_source(0) {
        Ok(source) => source,
        Err(()) => {
            handshake.report(Startup::Failed(format!(
                "failed to create run-loop source for {tap_label}"
            )));
            return;
        }
    };

    let run_loop = CFRunLoop::get_current();
    unsafe {
        run_loop.add_source(&source, kCFRunLoopCommonModes);
    }
    tap.enable();

    // Attach a one-shot `kCFRunLoopEntry` observer and only send the run loop
    // back to the caller from inside it, once the loop has actually started
    // running — see the module doc comment for why sending it any earlier
    // would let a `stop()` race the loop's entry and go missing.
    let ctx = EntryContext {
        handshake: Arc::clone(handshake),
        live: Arc::clone(&live),
    };
    let mut observer_context = CFRunLoopObserverContext {
        version: 0,
        info: &ctx as *const EntryContext as *mut c_void,
        retain: None,
        release: None,
        copyDescription: None,
    };
    let observer = unsafe {
        // Safety: `CFRunLoopObserverCreate` returns a `+1`-retained observer
        // ref, matching `wrap_under_create_rule`; `on_run_loop_entry` is a
        // valid `extern "C"` callback with the signature CF expects, and
        // `observer_context.info` points at `ctx`, which outlives the observer
        // (both are dropped only after `CFRunLoopRun` returns below, and the
        // observer never repeats so it fires, at most, before that point).
        CFRunLoopObserver::wrap_under_create_rule(CFRunLoopObserverCreate(
            core_foundation_sys::base::kCFAllocatorDefault,
            kCFRunLoopEntry,
            false as core_foundation_sys::base::Boolean,
            0,
            on_run_loop_entry,
            &mut observer_context,
        ))
    };
    run_loop.add_observer(&observer, unsafe { kCFRunLoopCommonModes });

    CFRunLoop::run_current();
    // Run loop stopped: returning here drops the locals in reverse declaration
    // order — `observer` (releasing it) before `ctx`, which is what keeps the
    // observer from outliving the context its `info` points at, and `tap` last
    // (invalidating the port).
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Long enough that a wrongly-unbounded wait is unmistakable, short enough
    /// not to slow the suite down.
    const PATIENCE: Duration = Duration::from_secs(5);
    /// Short enough that the "gave up" cases finish quickly.
    const BRIEF: Duration = Duration::from_millis(50);

    #[test]
    fn a_thread_that_returns_is_joined() {
        let handle = std::thread::spawn(|| {});
        assert!(join_bounded(handle, "test tap", PATIENCE));
    }

    #[test]
    fn a_thread_that_will_not_return_is_detached_at_the_deadline() {
        // Stands in for a callback stuck inside an OS call it cannot cancel: the
        // teardown has to give up on it rather than hang a settings save.
        let (release, blocked) = std::sync::mpsc::channel::<()>();
        let handle = std::thread::spawn(move || {
            let _ = blocked.recv();
        });
        let started = Instant::now();
        assert!(!join_bounded(handle, "test tap", BRIEF));
        assert!(started.elapsed() >= BRIEF, "it waited for the deadline");
        assert!(
            started.elapsed() < PATIENCE,
            "it did not wait past the deadline"
        );
        // Let the thread go so it does not outlive the test.
        let _ = release.send(());
    }

    #[test]
    fn the_handshake_hands_over_a_reported_failure() {
        let handshake = Handshake::default();
        assert!(handshake.report(Startup::Failed("no permission".into())));
        match handshake.wait(PATIENCE) {
            Startup::Failed(e) => assert_eq!(e, "no permission"),
            _ => panic!("the reported failure is what the caller sees"),
        }
    }

    #[test]
    fn the_handshake_gives_up_at_the_deadline_and_stays_given_up() {
        let handshake = Handshake::default();
        let started = Instant::now();
        assert!(matches!(handshake.wait(BRIEF), Startup::Waiting));
        assert!(started.elapsed() >= BRIEF);
        // The point of leaving `Abandoned` behind: a thread that reports after
        // the caller gave up is told so, and stops its run loop instead of
        // handling input with nobody holding it.
        assert!(!handshake.report(Startup::Failed("too late".into())));
    }

    #[test]
    fn the_handshake_wakes_the_caller_as_soon_as_a_thread_reports() {
        let handshake = Arc::new(Handshake::default());
        let reporter = Arc::clone(&handshake);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            reporter.report(Startup::Failed("reported late but in time".into()));
        });
        // Would fail by timing out into `Waiting` if the wait did not wake on
        // the report.
        assert!(matches!(handshake.wait(PATIENCE), Startup::Failed(_)));
    }
}
