//! A bounded, coalescing command queue between an event-tap callback and the
//! worker that acts on its commands.
//!
//! The drag taps hand every pointer event to a worker thread that talks to
//! the target window over Accessibility. With a plain unbounded channel the
//! callback keeps enqueuing while the worker is stuck in an AX call (each one
//! bounded, but a wedged target can eat the bound over and over), so the queue
//! grows for as long as the stall lasts and the worker then replays a backlog
//! of stale cursor positions.
//!
//! Commands travel two ways, by kind:
//!
//! * **Positional** updates (a cursor sample) are folded by the *producer*: a
//!   newer sample replaces the pending one for the same gesture, so at most one
//!   is held per gesture, and a hard cap refuses samples of further gestures
//!   beyond that. Their slot sits behind a lock the worker holds only to take
//!   one entry; the callback tries that lock a bounded number of times and, if
//!   the worker was descheduled mid-take, drops the sample rather than wait —
//!   the next one is milliseconds away and supersedes it anyway. Each pending
//!   entry is announced on the lifecycle channel by one `Tick`, queued when the
//!   entry is created, so a gesture's samples are handed out after its press
//!   and before its release even while an older gesture's are still pending.
//! * **Lifecycle** commands (press, release, cancel; begin, end) travel a
//!   lock-free channel and are never shed: the worker needs every one of them
//!   to end a gesture cleanly. They are bounded by physical clicks, not by the
//!   event rate, so leaving that channel unbounded costs nothing.
//!
//! So the callback never blocks on the worker, whatever state it is in. What
//! was shed is counted and reported from the worker's side, so backpressure is
//! visible rather than silent and the callback never writes a log line.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use crate::locks::MutexExt;

/// How a command type folds into the queue.
pub trait Coalesce {
    /// Whether `self` makes `earlier` redundant — the same gesture's newer
    /// cursor position, say. `earlier` is then replaced in place.
    fn supersedes(&self, earlier: &Self) -> bool;
    /// Whether the command is a positional update (coalesced, and dropped
    /// under pressure) rather than a lifecycle command (always delivered).
    fn sheddable(&self) -> bool;
}

/// Upper bound on pending positional commands. One per gesture in the steady
/// state; the cap only matters when gestures pile up behind a stalled worker.
const CAPACITY: usize = 64;

/// How many times `send` re-tries the positional slot's lock before dropping
/// the sample. Each attempt is a failed `try_lock` plus a spin hint —
/// nanoseconds — so the whole budget is a few microseconds: enough to ride out
/// the worker's drain (a `VecDeque` swap under the lock), far too little to
/// stall the tap callback the caller runs on.
const TRY_LOCK_ATTEMPTS: u32 = 64;

enum Msg<C> {
    /// A lifecycle command.
    Command(C),
    /// One entry was added to the positional slot; take the oldest.
    Tick,
}

struct Shared<C> {
    /// Pending positional commands, one per gesture, oldest first. Invariant:
    /// every entry has exactly one `Tick` queued on the lifecycle channel
    /// behind it, so the receiver takes entries in the order they were created.
    positions: Mutex<VecDeque<C>>,
    /// Samples dropped (at the cap, or to a contended lock) since the receiver
    /// last reported; the receiver takes it with `swap(0)`.
    shed: AtomicU32,
    /// Running total, for the log; wraps.
    shed_total: AtomicU32,
    label: &'static str,
}

/// The producer end. Deliberately not `Clone`: the "one tick per pending
/// entry" ordering below relies on a single producer creating entries and
/// queuing their ticks in the same order, and every user of this queue is one
/// tap callback. The queue closes when it drops.
pub struct Sender<C> {
    shared: Arc<Shared<C>>,
    lifecycle: mpsc::Sender<Msg<C>>,
}

/// The consumer end.
pub struct Receiver<C> {
    shared: Arc<Shared<C>>,
    lifecycle: mpsc::Receiver<Msg<C>>,
}

pub fn channel<C: Coalesce>(label: &'static str) -> (Sender<C>, Receiver<C>) {
    let shared = Arc::new(Shared {
        positions: Mutex::new(VecDeque::new()),
        shed: AtomicU32::new(0),
        shed_total: AtomicU32::new(0),
        label,
    });
    let (tx, rx) = mpsc::channel();
    (
        Sender {
            shared: Arc::clone(&shared),
            lifecycle: tx,
        },
        Receiver {
            shared,
            lifecycle: rx,
        },
    )
}

impl<C: Coalesce> Sender<C> {
    /// Queue `command`. Never blocks on the worker (see the module doc); does
    /// no logging or I/O, since the caller is an event-tap callback. Returns
    /// `false` when the receiver is gone.
    pub fn send(&self, command: C) -> bool {
        if !command.sheddable() {
            return self.lifecycle.send(Msg::Command(command)).is_ok();
        }
        let Some(mut positions) = try_lock_briefly(&self.shared.positions) else {
            self.shared.shed.fetch_add(1, Ordering::Relaxed);
            return true;
        };
        if let Some(pos) = positions.iter().rposition(|e| command.supersedes(e)) {
            // The entry's tick is already queued; the newer sample just takes
            // its place.
            positions[pos] = command;
            return true;
        }
        if positions.len() >= CAPACITY {
            // A backlog of this many gestures will never be caught up with;
            // refusing the new sample (rather than shedding the oldest entry)
            // keeps every pending entry paired with its tick.
            self.shared.shed.fetch_add(1, Ordering::Relaxed);
            return true;
        }
        positions.push_back(command);
        drop(positions);
        self.lifecycle.send(Msg::Tick).is_ok()
    }
}

/// `try_lock`, retried a bounded number of times. `None` if the lock stayed
/// contended throughout.
fn try_lock_briefly<T>(mutex: &Mutex<T>) -> Option<std::sync::MutexGuard<'_, T>> {
    for _ in 0..TRY_LOCK_ATTEMPTS {
        match mutex.try_lock() {
            Ok(guard) => return Some(guard),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => return Some(poisoned.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => std::hint::spin_loop(),
        }
    }
    None
}

impl<C> Receiver<C> {
    /// Wait for the next command. `None` once every sender is gone and
    /// everything queued has been handed out.
    pub fn recv(&mut self) -> Option<C> {
        loop {
            match self.lifecycle.recv() {
                Ok(Msg::Command(command)) => {
                    self.report_shed();
                    return Some(command);
                }
                Ok(Msg::Tick) => {
                    if let Some(position) = self.take_position() {
                        return Some(position);
                    }
                }
                Err(_) => {
                    // Every sender is gone; a tick for anything still in the
                    // slot went with them.
                    self.report_shed();
                    return self.take_position();
                }
            }
        }
    }

    /// The next command if one is queued, without waiting.
    pub fn try_recv(&mut self) -> Option<C> {
        loop {
            match self.lifecycle.try_recv() {
                Ok(Msg::Command(command)) => {
                    self.report_shed();
                    return Some(command);
                }
                Ok(Msg::Tick) => {
                    if let Some(position) = self.take_position() {
                        return Some(position);
                    }
                }
                Err(_) => {
                    self.report_shed();
                    return None;
                }
            }
        }
    }

    /// Take the oldest pending positional entry — the one the tick just
    /// received was queued for. The lock is held only for the pop.
    fn take_position(&mut self) -> Option<C> {
        self.shared.positions.lock_safe().pop_front()
    }

    /// Log, from the consumer's thread and outside the lock, whatever the
    /// producer had to shed since the last report.
    fn report_shed(&self) {
        let since = self.shared.shed.swap(0, Ordering::Relaxed);
        if since == 0 {
            return;
        }
        let total = self
            .shared
            .shed_total
            .fetch_add(since, Ordering::Relaxed)
            .wrapping_add(since);
        tracing::warn!(
            queue = self.shared.label,
            shed_since_last_report = since,
            shed_total = total,
            "command queue dropped cursor samples; the worker is not keeping up"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Cmd {
        Begin(u64),
        Move(u64, i32),
        End(u64),
    }

    impl Coalesce for Cmd {
        fn supersedes(&self, earlier: &Self) -> bool {
            matches!((self, earlier), (Cmd::Move(g, _), Cmd::Move(h, _)) if g == h)
        }
        fn sheddable(&self) -> bool {
            matches!(self, Cmd::Move(..))
        }
    }

    fn drain(rx: &mut Receiver<Cmd>) -> Vec<Cmd> {
        std::iter::from_fn(|| rx.try_recv()).collect()
    }

    #[test]
    fn a_newer_position_replaces_the_pending_one_and_keeps_its_place() {
        let (tx, mut rx) = channel::<Cmd>("test");
        tx.send(Cmd::Begin(1));
        tx.send(Cmd::Move(1, 10));
        tx.send(Cmd::Move(1, 20));
        tx.send(Cmd::End(1));
        // Still the same gesture's slot, drained where the first sample queued
        // its tick — before the End that followed it.
        tx.send(Cmd::Move(1, 30));
        assert_eq!(
            drain(&mut rx),
            vec![Cmd::Begin(1), Cmd::Move(1, 30), Cmd::End(1)]
        );
    }

    #[test]
    fn positions_of_different_gestures_do_not_fold_into_each_other() {
        let (tx, mut rx) = channel::<Cmd>("test");
        tx.send(Cmd::Move(1, 10));
        tx.send(Cmd::Move(2, 20));
        assert_eq!(drain(&mut rx), vec![Cmd::Move(1, 10), Cmd::Move(2, 20)]);
    }

    #[test]
    fn samples_of_a_later_gesture_stay_behind_its_press_and_the_earlier_release() {
        // The worker is behind: two whole gestures queue up. The second one's
        // sample must not be handed out ahead of the first one's release (the
        // absorb step would then discard it as belonging to an unknown
        // generation and the second drag would move nothing).
        let (tx, mut rx) = channel::<Cmd>("test");
        tx.send(Cmd::Begin(1));
        tx.send(Cmd::Move(1, 10));
        tx.send(Cmd::End(1));
        tx.send(Cmd::Begin(2));
        tx.send(Cmd::Move(2, 20));
        tx.send(Cmd::Move(2, 21));
        tx.send(Cmd::End(2));
        assert_eq!(
            drain(&mut rx),
            vec![
                Cmd::Begin(1),
                Cmd::Move(1, 10),
                Cmd::End(1),
                Cmd::Begin(2),
                Cmd::Move(2, 21),
                Cmd::End(2),
            ]
        );
    }

    #[test]
    fn the_cap_refuses_samples_of_further_gestures_and_never_a_lifecycle_command() {
        let (tx, mut rx) = channel::<Cmd>("test");
        for g in 0..(CAPACITY as u64 + 2) {
            tx.send(Cmd::Begin(g));
            tx.send(Cmd::Move(g, 0));
            tx.send(Cmd::End(g));
        }
        // A pending gesture's newer sample still replaces in place at the cap.
        tx.send(Cmd::Move(3, 7));
        let items = drain(&mut rx);
        let positions = items.iter().filter(|c| matches!(c, Cmd::Move(..))).count();
        let lifecycle = items.len() - positions;
        assert_eq!(positions, CAPACITY);
        assert_eq!(lifecycle, 2 * (CAPACITY + 2));
        assert!(items.contains(&Cmd::Move(0, 0)));
        assert!(items.contains(&Cmd::Move(3, 7)));
        assert!(!items.contains(&Cmd::Move(CAPACITY as u64, 0)));
        assert!(!items.contains(&Cmd::Move(CAPACITY as u64 + 1, 0)));
        // Every sample still sits between its own press and release.
        for (i, item) in items.iter().enumerate() {
            if let Cmd::Move(g, _) = item {
                assert_eq!(items[i - 1], Cmd::Begin(*g));
                assert_eq!(items[i + 1], Cmd::End(*g));
            }
        }
    }

    #[test]
    fn recv_returns_none_once_the_sender_is_gone_and_the_queue_is_drained() {
        let (tx, mut rx) = channel::<Cmd>("test");
        tx.send(Cmd::Begin(1));
        tx.send(Cmd::Move(1, 5));
        assert_eq!(rx.recv(), Some(Cmd::Begin(1)));
        let waiter = std::thread::spawn(move || (rx.recv(), rx.recv(), rx.recv()));
        std::thread::sleep(std::time::Duration::from_millis(50));
        drop(tx);
        let (a, b, c) = waiter.join().unwrap();
        assert_eq!(a, Some(Cmd::Move(1, 5)));
        assert_eq!(b, None);
        assert_eq!(c, None);
    }

    #[test]
    fn a_contended_slot_drops_a_position_while_a_lifecycle_command_gets_through() {
        let (tx, mut rx) = channel::<Cmd>("test");
        // Pin the positional slot's lock from another thread until told to
        // let go, so neither send below can be waiting on it when it returns.
        let shared = Arc::clone(&tx.shared);
        let (locked_tx, locked_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let holder = std::thread::spawn(move || {
            let guard = shared.positions.lock_safe();
            locked_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            drop(guard);
        });
        locked_rx.recv().unwrap();

        // Both return while the lock is still held: the sample is dropped and
        // counted, the lifecycle command lands on its lock-free channel.
        assert!(tx.send(Cmd::Move(1, 1)));
        assert_eq!(tx.shared.shed.load(Ordering::Relaxed), 1);
        assert!(tx.send(Cmd::End(1)));
        assert_eq!(rx.try_recv(), Some(Cmd::End(1)));

        release_tx.send(()).unwrap();
        holder.join().unwrap();
        assert_eq!(drain(&mut rx), Vec::<Cmd>::new());
    }

    #[test]
    fn send_reports_a_dropped_receiver() {
        let (tx, rx) = channel::<Cmd>("test");
        assert!(tx.send(Cmd::Begin(1)));
        drop(rx);
        assert!(!tx.send(Cmd::Begin(2)));
    }
}
