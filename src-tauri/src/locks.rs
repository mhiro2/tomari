//! Poison-tolerant mutex locking for the resident app.
//!
//! A panic while a lock is held *poisons* it; thereafter the default
//! `.lock().unwrap()` panics on every acquisition. With `panic = "abort"` in the
//! release profile that turns one stray panic into an immediate, silent
//! termination of a background menu-bar app — and even in debug it cascades a
//! single failure across every thread that touches the lock.
//!
//! A resident app should instead recover the guarded value and carry on, so the
//! runtime lock sites go through [`MutexExt::lock_safe`], which takes the inner
//! guard back from a poisoned lock rather than propagating the poison. The data
//! a panicking thread left behind may be momentarily inconsistent, but every
//! value guarded this way is a small, self-consistent cache (settings, the rule
//! engine, the shortcut map, …) that the next write fully replaces — far better
//! than killing the process.

use std::sync::{Mutex, MutexGuard};

pub trait MutexExt<T> {
    /// Lock, recovering the guard if the mutex was poisoned by a panic in
    /// another thread instead of panicking in turn.
    fn lock_safe(&self) -> MutexGuard<'_, T>;
    /// Try to lock without waiting, with the same poison recovery. `None` only
    /// when another thread holds the lock.
    fn try_lock_safe(&self) -> Option<MutexGuard<'_, T>>;
}

impl<T> MutexExt<T> for Mutex<T> {
    fn lock_safe(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
    fn try_lock_safe(&self) -> Option<MutexGuard<'_, T>> {
        match self.try_lock() {
            Ok(guard) => Some(guard),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => None,
        }
    }
}
