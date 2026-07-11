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
//! before the loop is entered, and the run loop handle is only sent to the
//! caller from *inside* that observer's callback — i.e. only once the run loop
//! has actually started running. Any `stop()` the caller issues after `recv()`
//! returns is therefore guaranteed to land on a running loop and take effect
//! immediately, instead of a periodic-wakeup workaround (an `AtomicBool` flag
//! polled through a bounded `CFRunLoopRunInMode` loop), which would burn a
//! wakeup every tick even while fully idle.
use std::os::raw::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;

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

/// A running event tap: the run loop it is attached to (so it can be stopped)
/// and the thread driving it. Shared by all three taps; dropping it stops the
/// run loop and joins the thread, invalidating the tap.
pub struct RunningTap {
    run_loop: CFRunLoop,
    thread: Option<JoinHandle<()>>,
}

impl Drop for RunningTap {
    fn drop(&mut self) {
        // Stopping the run loop makes `CFRunLoopRun` return; the thread then
        // drops the tap (invalidating it) and exits. Safe to call unconditionally
        // because `spawn` only ever hands out a `RunningTap` whose run loop has
        // already entered `CFRunLoopRun` (see the module doc comment) — so this
        // `stop()` is never a no-op racing against a not-yet-running loop.
        self.run_loop.stop();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
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
    let (tx, rx) = std::sync::mpsc::channel();
    let thread = std::thread::Builder::new()
        .name(thread_name.into())
        .spawn(move || run_tap(tap_label, options, events, make_callback, tx))
        .map_err(|e| e.to_string())?;

    match rx.recv() {
        Ok(Ok(run_loop)) => Ok(RunningTap {
            run_loop,
            thread: Some(thread),
        }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(e) => Err(format!("{tap_label} thread exited before signalling: {e}")),
    }
}

/// Context handed to the `kCFRunLoopEntry` observer callback: where to send the
/// now-running run loop.
struct EntryContext {
    tx: Sender<Result<CFRunLoop, String>>,
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
    // entry into the current pass), so it is now running — a `stop()` a
    // caller performs after receiving this will always land on a live loop
    // rather than racing its entry (see the module doc comment).
    let _ = ctx.tx.send(Ok(CFRunLoop::get_current()));
}

fn run_tap<F>(
    tap_label: &'static str,
    options: CGEventTapOptions,
    events: Vec<CGEventType>,
    make_callback: F,
    tx: Sender<Result<CFRunLoop, String>>,
) where
    F: FnOnce(Arc<AtomicUsize>) -> TapCallback + Send + 'static,
{
    let port_holder = Arc::new(AtomicUsize::new(0));
    let callback = make_callback(port_holder.clone());

    let tap = match CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        options,
        events,
        move |proxy, etype, event| callback(proxy, etype, event),
    ) {
        Ok(tap) => tap,
        Err(()) => {
            let _ = tx.send(Err(format!(
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
            let _ = tx.send(Err(format!(
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
    let ctx = EntryContext { tx };
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
    // Run loop stopped: returning here drops `tap` (invalidating the port),
    // `observer` (removing/releasing it) and `ctx`, in that order.
}
