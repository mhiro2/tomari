//! Re-apply the Caps Lock HID remap when a keyboard is connected or removed.
//!
//! `hidutil property --matching` writes `UserKeyMapping` onto the keyboard
//! services that exist *at that moment*. A keyboard that appears later (USB
//! plugged in, Bluetooth reconnecting) gets no mapping, and a removed service
//! takes its mapping with it (Apple TN2450), so Caps Lock would go back to being
//! a native lock key on that device. Launch, saves, the wake reset and the
//! permission poll already reconcile; this module adds the missing trigger:
//! IOKit service match / termination notifications for HID keyboards, plus one
//! reconcile once the watch is armed to cover a keyboard connected between the
//! launch reconcile and the registration.
//!
//! IOKit matching notifications need no TCC (Input Monitoring) grant, unlike
//! `IOHIDManagerOpen`, which can raise a permission prompt — so the manager API
//! is deliberately not used here.
//!
//! The matching dictionary is `IOHIDDevice` with `DeviceUsagePage = 1` and
//! `DeviceUsage = 6` (Generic Desktop / Keyboard). `IOHIDDevice` publishes those
//! as properties and its property-table match understands them, which is the
//! same filter `hidutil list --matching` applies in `capsmap::keyboards`. The
//! `#[ignore]`d `matching_dictionary_finds_a_keyboard` test checks it against a
//! real machine; if a setup ever fails to match, the symptom is only a missed
//! hot-plug reconcile, never a wrong remap.
//!
//! The notification callback runs on a dedicated run-loop thread and only
//! forwards a token over a channel. A second thread debounces the bursts (one
//! plug-in publishes several services) and runs the reconcile through
//! [`crate::eventtap::reconcile_caps_mapping`], which owns the lifecycle gate,
//! the tap lock and the Caps Lock-held deferral.

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// Quiet period after the last notification before reconciling.
const QUIET: Duration = Duration::from_millis(800);
/// A service can be published after the `IOHIDDevice` appears, so the first
/// reconcile may not see it yet; one more runs this long after it.
const RETRY_AFTER: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stage {
    First,
    Retry,
}

/// Coalesces notification bursts into one reconcile, plus one delayed retry.
#[derive(Debug)]
struct Debouncer {
    quiet: Duration,
    retry_after: Duration,
    deadline: Option<Instant>,
    stage: Stage,
}

impl Debouncer {
    fn new(quiet: Duration, retry_after: Duration) -> Self {
        Self {
            quiet,
            retry_after,
            deadline: None,
            stage: Stage::First,
        }
    }

    /// A notification arrived: restart the quiet period.
    fn note(&mut self, now: Instant) {
        self.deadline = Some(now + self.quiet);
        self.stage = Stage::First;
    }

    /// When the next reconcile is due, if any is pending.
    fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// Whether a reconcile should run now. After the first one, a single retry
    /// is scheduled; a new notification starts over.
    fn due(&mut self, now: Instant) -> bool {
        match self.deadline {
            Some(at) if at <= now => {
                match self.stage {
                    Stage::First => {
                        self.stage = Stage::Retry;
                        self.deadline = Some(now + self.retry_after);
                    }
                    Stage::Retry => self.deadline = None,
                }
                true
            }
            _ => false,
        }
    }
}

/// Wait for notifications and call `trigger` for each debounced burst. Returns
/// when the sender side is gone. Nothing is triggered while `is_running` is
/// false, and pending work is dropped then.
fn worker_loop(
    rx: &Receiver<()>,
    mut debouncer: Debouncer,
    is_running: impl Fn() -> bool,
    mut trigger: impl FnMut(),
) {
    loop {
        let received = match debouncer.deadline() {
            Some(at) => rx.recv_timeout(at.saturating_duration_since(Instant::now())),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match received {
            Ok(()) => {
                if is_running() {
                    debouncer.note(Instant::now());
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if !is_running() {
                    debouncer.deadline = None;
                } else if debouncer.due(Instant::now()) {
                    trigger();
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Release every object the iterator yields, returning how many there were.
fn drain_all(mut next: impl FnMut() -> u32, mut release: impl FnMut(u32)) -> usize {
    let mut count = 0;
    loop {
        let object = next();
        if object == 0 {
            return count;
        }
        release(object);
        count += 1;
    }
}

#[cfg(target_os = "macos")]
pub use imp::install;

#[cfg(target_os = "macos")]
mod imp {
    use std::os::raw::{c_char, c_void};
    use std::sync::mpsc::{Sender, channel};

    use core_foundation::base::TCFType;
    use core_foundation::number::CFNumber;
    use core_foundation::runloop::{CFRunLoop, CFRunLoopSource, kCFRunLoopDefaultMode};
    use core_foundation::string::CFString;
    use core_foundation_sys::dictionary::{CFDictionarySetValue, CFMutableDictionaryRef};
    use core_foundation_sys::runloop::CFRunLoopSourceRef;
    use tauri::{AppHandle, Manager};

    use super::{Debouncer, QUIET, RETRY_AFTER, drain_all, worker_loop};
    use crate::state::AppState;

    #[allow(non_camel_case_types)]
    type io_object_t = u32;
    #[allow(non_camel_case_types)]
    type io_iterator_t = u32;
    type IONotificationPortRef = *mut c_void;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IONotificationPortCreate(main_port: u32) -> IONotificationPortRef;
        fn IONotificationPortDestroy(port: IONotificationPortRef);
        fn IONotificationPortGetRunLoopSource(port: IONotificationPortRef) -> CFRunLoopSourceRef;
        fn IOServiceMatching(name: *const c_char) -> CFMutableDictionaryRef;
        fn IOServiceAddMatchingNotification(
            port: IONotificationPortRef,
            notification_type: *const c_char,
            matching: CFMutableDictionaryRef,
            callback: extern "C" fn(*mut c_void, io_iterator_t),
            refcon: *mut c_void,
            notification: *mut io_iterator_t,
        ) -> i32;
        #[cfg(test)]
        fn IOServiceGetMatchingServices(
            main_port: u32,
            matching: CFMutableDictionaryRef,
            existing: *mut io_iterator_t,
        ) -> i32;
        fn IOIteratorNext(iterator: io_iterator_t) -> io_object_t;
        fn IOObjectRelease(object: io_object_t) -> i32;
    }

    /// `kIOFirstMatchNotification` / `kIOTerminatedNotification`.
    const FIRST_MATCH: &[u8] = b"IOServiceFirstMatch\0";
    const TERMINATED: &[u8] = b"IOServiceTerminate\0";

    /// A fresh matching dictionary for HID keyboards (see the module docs).
    /// Each IOKit call that takes a dictionary consumes it, so callers build one
    /// per call.
    fn keyboard_matching() -> CFMutableDictionaryRef {
        // SAFETY: the class name is NUL-terminated. The result follows the
        // Create rule and is handed to IOKit, which consumes it.
        let dict = unsafe { IOServiceMatching(c"IOHIDDevice".as_ptr()) };
        if dict.is_null() {
            return dict;
        }
        for (key, value) in [("DeviceUsagePage", 1i32), ("DeviceUsage", 6i32)] {
            let key = CFString::new(key);
            let value = CFNumber::from(value);
            // SAFETY: the dictionary is valid and mutable; CFDictionary retains
            // both the key and the value, so dropping our wrappers is fine.
            unsafe {
                CFDictionarySetValue(
                    dict,
                    key.as_concrete_TypeRef().cast(),
                    value.as_concrete_TypeRef().cast(),
                );
            }
        }
        dict
    }

    fn drain(iterator: io_iterator_t) -> usize {
        // SAFETY: `iterator` is a live iterator from IOKit and each object it
        // yields is owned by us until released.
        drain_all(
            || unsafe { IOIteratorNext(iterator) },
            |object| {
                unsafe { IOObjectRelease(object) };
            },
        )
    }

    /// Run-loop callback: drain (which re-arms the notification) and forward a
    /// token if anything changed. No process is spawned here.
    extern "C" fn on_keyboards_changed(refcon: *mut c_void, iterator: io_iterator_t) {
        let count = drain(iterator);
        if count == 0 {
            return;
        }
        // SAFETY: `refcon` is the leaked `Sender` box created in `run_loop`,
        // which is never freed, so it outlives every callback.
        let tx = unsafe { &*(refcon as *const Sender<()>) };
        let _ = tx.send(());
    }

    /// Observe keyboard arrival and removal for the app's lifetime. Best
    /// effort: failures are logged and the other reconcile triggers remain.
    pub fn install(app: &AppHandle) {
        let (tx, rx) = channel::<()>();
        let handle = app.clone();
        let worker = std::thread::Builder::new()
            .name("tomari-keyboard-hotplug".into())
            .spawn(move || {
                worker_loop(
                    &rx,
                    Debouncer::new(QUIET, RETRY_AFTER),
                    || {
                        handle
                            .try_state::<AppState>()
                            .is_some_and(|state| state.lifecycle.is_running())
                    },
                    || spawn_reconcile(&handle),
                );
            });
        if let Err(e) = worker {
            tracing::warn!(error = %e, "could not start the keyboard hot-plug worker");
            return;
        }
        let spawned = std::thread::Builder::new()
            .name("tomari-keyboard-notify".into())
            .spawn(move || run_loop(tx));
        if let Err(e) = spawned {
            tracing::warn!(error = %e, "could not start the keyboard notification thread");
        }
    }

    /// Register one keyboard notification on `port` and drain its initial
    /// iterator, which lists keyboards that already exist and arms it. The
    /// iterator is kept for the process lifetime: releasing it would cancel the
    /// notification.
    fn register(port: IONotificationPortRef, kind: &[u8], refcon: *mut c_void) -> Result<(), ()> {
        let matching = keyboard_matching();
        if matching.is_null() {
            tracing::warn!("could not build the keyboard matching dictionary");
            return Err(());
        }
        let mut iterator: io_iterator_t = 0;
        // SAFETY: `kind` is NUL-terminated, the dictionary is consumed by the
        // call whether it succeeds or not, and the callback matches the
        // expected signature.
        let rc = unsafe {
            IOServiceAddMatchingNotification(
                port,
                kind.as_ptr().cast(),
                matching,
                on_keyboards_changed,
                refcon,
                &mut iterator,
            )
        };
        if rc != 0 {
            tracing::warn!(rc, "could not register for keyboard notifications");
            return Err(());
        }
        drain(iterator);
        Ok(())
    }

    fn run_loop(tx: Sender<()>) {
        // SAFETY: 0 is kIOMainPortDefault. Once a notification is registered
        // the port is kept for the process lifetime (never destroyed), as is
        // its run-loop source.
        let port = unsafe { IONotificationPortCreate(0) };
        if port.is_null() {
            // Dropping `tx` lets the worker see the disconnect and exit.
            tracing::warn!("IOKit notification port unavailable; keyboard hot-plug is not watched");
            return;
        }
        let source = unsafe { IONotificationPortGetRunLoopSource(port) };
        if source.is_null() {
            unsafe { IONotificationPortDestroy(port) };
            tracing::warn!("IOKit offered no run-loop source; keyboard hot-plug is not watched");
            return;
        }
        // The source is owned by the port (Get rule), so retain before wrapping.
        let source = unsafe { CFRunLoopSource::wrap_under_get_rule(source) };
        let run_loop = CFRunLoop::get_current();
        unsafe { run_loop.add_source(&source, kCFRunLoopDefaultMode) };

        // Handed to IOKit as the callbacks' context. Reclaimed below if the
        // arrival watch cannot be registered; otherwise leaked, because the
        // callbacks reference it until the process exits.
        let refcon = Box::into_raw(Box::new(tx.clone())).cast::<c_void>();
        // Arrival is what this watch exists for, so it is registered first and
        // a failure there abandons the watch with nothing left behind. Removal
        // only lets a reconcile drop a gone keyboard's claim early (the next
        // reconcile of any kind does that too), so a failure there is logged
        // and the arrival watch is kept.
        for kind in [FIRST_MATCH, TERMINATED] {
            let registered = register(port, kind, refcon);
            if registered.is_err() && kind == FIRST_MATCH {
                unsafe { run_loop.remove_source(&source, kCFRunLoopDefaultMode) };
                // SAFETY: no registration holds `refcon` or the port, so both
                // can be reclaimed; dropping the senders disconnects the worker.
                unsafe {
                    drop(Box::from_raw(refcon.cast::<Sender<()>>()));
                    IONotificationPortDestroy(port);
                }
                tracing::warn!("keyboard hot-plug is not watched");
                return;
            }
        }
        // A keyboard connected between the launch reconcile and this point is
        // in the initial iterator rather than a notification, so reconcile
        // once now that the watch is armed. Already-mapped keyboards make it
        // a no-op.
        let _ = tx.send(());
        drop(tx);
        CFRunLoop::run_current();
    }

    fn spawn_reconcile(app: &AppHandle) {
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        let lifecycle = std::sync::Arc::clone(&state.lifecycle);
        let handle = app.clone();
        let spawned = lifecycle.spawn_tracked("tomari-keyboard-hotplug-reconcile", move |_| {
            let Some(state) = handle.try_state::<AppState>() else {
                return;
            };
            if crate::eventtap::reconcile_caps_mapping(&state) {
                tracing::info!("caps-lock HID remap re-applied after a keyboard change");
            } else {
                tracing::warn!(
                    "caps-lock HID remap reconcile after a keyboard change did not match the rules"
                );
            }
        });
        if let Err(e) = spawned {
            tracing::warn!(error = %e, "could not spawn the keyboard hot-plug reconcile");
        }
    }

    /// Count the services the keyboard matching dictionary finds right now.
    #[cfg(test)]
    pub(super) fn matching_keyboard_count() -> usize {
        let mut iterator: io_iterator_t = 0;
        let matching = keyboard_matching();
        assert!(!matching.is_null());
        let rc = unsafe { IOServiceGetMatchingServices(0, matching, &mut iterator) };
        assert_eq!(rc, 0);
        let count = drain(iterator);
        unsafe { IOObjectRelease(iterator) };
        count
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, mpsc};

    use super::*;

    #[test]
    fn a_burst_coalesces_into_one_reconcile_then_one_retry() {
        let t0 = Instant::now();
        let mut d = Debouncer::new(Duration::from_millis(100), Duration::from_millis(500));
        assert!(!d.due(t0), "nothing pending without a notification");
        d.note(t0);
        d.note(t0 + Duration::from_millis(60));
        assert!(
            !d.due(t0 + Duration::from_millis(120)),
            "burst restarts the quiet period"
        );
        assert!(d.due(t0 + Duration::from_millis(160)));
        assert!(!d.due(t0 + Duration::from_millis(300)));
        assert!(
            d.due(t0 + Duration::from_millis(700)),
            "single delayed retry"
        );
        assert!(!d.due(t0 + Duration::from_secs(60)));
        assert_eq!(d.deadline(), None);
    }

    #[test]
    fn a_new_notification_cancels_a_pending_retry() {
        let t0 = Instant::now();
        let mut d = Debouncer::new(Duration::from_millis(100), Duration::from_millis(500));
        d.note(t0);
        assert!(d.due(t0 + Duration::from_millis(100)));
        d.note(t0 + Duration::from_millis(200));
        assert!(d.due(t0 + Duration::from_millis(300)));
    }

    #[test]
    fn drain_releases_every_object_and_counts_them() {
        let mut queue = vec![7u32, 5, 3];
        let released = RefCell::new(Vec::new());
        let n = drain_all(
            || queue.pop().unwrap_or(0),
            |o| released.borrow_mut().push(o),
        );
        assert_eq!(n, 3);
        assert_eq!(released.into_inner(), vec![3, 5, 7]);
        assert_eq!(drain_all(|| 0, |_| panic!("nothing to release")), 0);
    }

    fn run_worker(running: bool, notify: bool) -> usize {
        let (tx, rx) = mpsc::channel();
        let runs = Arc::new(AtomicUsize::new(0));
        let flag = Arc::new(AtomicBool::new(running));
        let r = Arc::clone(&runs);
        let f = Arc::clone(&flag);
        let worker = std::thread::spawn(move || {
            worker_loop(
                &rx,
                Debouncer::new(Duration::from_millis(30), Duration::from_millis(60)),
                move || f.load(Ordering::SeqCst),
                move || {
                    r.fetch_add(1, Ordering::SeqCst);
                },
            );
        });
        if notify {
            for _ in 0..3 {
                tx.send(()).unwrap();
            }
        }
        std::thread::sleep(Duration::from_millis(400));
        drop(tx);
        worker.join().unwrap();
        runs.load(Ordering::SeqCst)
    }

    #[test]
    fn worker_runs_one_reconcile_and_one_retry_per_burst() {
        assert_eq!(run_worker(true, true), 2);
    }

    #[test]
    fn worker_does_nothing_without_a_notification() {
        assert_eq!(run_worker(true, false), 0);
    }

    #[test]
    fn worker_does_not_trigger_after_shutdown() {
        assert_eq!(run_worker(false, true), 0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "needs a real machine with a keyboard attached"]
    fn matching_dictionary_finds_a_keyboard() {
        let n = imp::matching_keyboard_count();
        println!("matched keyboard services: {n}");
        assert!(n >= 1);
    }
}
