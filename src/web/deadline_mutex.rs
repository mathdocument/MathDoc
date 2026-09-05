//! Blocking mutex waits with deadlines and notification on unlock.
//!
//! Used only inside blocking API tasks. All guards notify on release, including
//! during unwinding, so timed waiters need neither polling nor an async runtime.
use std::ops::{Deref, DerefMut};
use std::sync::{Condvar, Mutex, MutexGuard, TryLockError};
use std::time::Instant;

pub(super) struct DeadlineMutex<T> {
    value: Mutex<T>,
    waiters: Mutex<()>,
    available: Condvar,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum LockError {
    TimedOut,
    Poisoned,
}

pub(super) struct Guard<'a, T> {
    mutex: &'a DeadlineMutex<T>,
    value: Option<MutexGuard<'a, T>>,
}

impl<T> DeadlineMutex<T> {
    pub(super) fn new(value: T) -> Self {
        Self {
            value: Mutex::new(value),
            waiters: Mutex::new(()),
            available: Condvar::new(),
        }
    }

    pub(super) fn lock_until(&self, deadline: Instant) -> Result<Guard<'_, T>, LockError> {
        // Avoid the notification gate in the uncontended case.
        if Instant::now() >= deadline {
            return Err(LockError::TimedOut);
        }
        match self.value.try_lock() {
            Ok(value) => return Ok(self.guard(value)),
            Err(TryLockError::Poisoned(_)) => return Err(LockError::Poisoned),
            Err(TryLockError::WouldBlock) => {}
        }

        let mut waiting = self.waiters.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if Instant::now() >= deadline {
                // A notified waiter may reach its deadline before being
                // scheduled. Pass the wakeup on rather than leave a free lock
                // with every remaining waiter asleep.
                self.available.notify_one();
                return Err(LockError::TimedOut);
            }
            // Recheck while holding the notification gate. Unlock takes the
            // same gate, preventing a notification between this check and wait.
            match self.value.try_lock() {
                Ok(value) => return Ok(self.guard(value)),
                Err(TryLockError::Poisoned(_)) => return Err(LockError::Poisoned),
                Err(TryLockError::WouldBlock) => {}
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            (waiting, _) = self
                .available
                .wait_timeout(waiting, remaining)
                .unwrap_or_else(|e| e.into_inner());
        }
    }

    fn guard<'a>(&'a self, value: MutexGuard<'a, T>) -> Guard<'a, T> {
        Guard {
            mutex: self,
            value: Some(value),
        }
    }

    #[cfg(test)]
    pub(super) fn lock(&self) -> std::sync::LockResult<Guard<'_, T>> {
        self.value
            .lock()
            .map(|value| self.guard(value))
            .map_err(|e| std::sync::PoisonError::new(self.guard(e.into_inner())))
    }

    #[cfg(test)]
    pub(super) fn try_lock(&self) -> std::sync::TryLockResult<Guard<'_, T>> {
        match self.value.try_lock() {
            Ok(value) => Ok(self.guard(value)),
            Err(TryLockError::WouldBlock) => Err(TryLockError::WouldBlock),
            Err(TryLockError::Poisoned(e)) => Err(TryLockError::Poisoned(
                std::sync::PoisonError::new(self.guard(e.into_inner())),
            )),
        }
    }
}

impl<T> Deref for Guard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.value.as_deref().expect("guard is held until drop")
    }
}

impl<T> DerefMut for Guard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.value.as_deref_mut().expect("guard is held until drop")
    }
}

impl<T> Drop for Guard<'_, T> {
    fn drop(&mut self) {
        let _waiting = self.mutex.waiters.lock().unwrap_or_else(|e| e.into_inner());
        drop(self.value.take());
        // Every waiter must observe poison promptly, since failed acquisitions
        // do not create guards that would otherwise notify the next waiter.
        if self.mutex.value.is_poisoned() {
            self.mutex.available.notify_all();
        } else {
            self.mutex.available.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::time::Duration;

    #[test]
    fn expired_deadline_does_not_acquire_an_available_lock() {
        let mutex = DeadlineMutex::new(0);
        assert!(matches!(
            mutex.lock_until(Instant::now()),
            Err(LockError::TimedOut)
        ));
    }

    #[test]
    fn contended_wait_times_out_and_later_acquisition_still_succeeds() {
        let mutex = DeadlineMutex::new(0);
        let guard = mutex.lock().unwrap();
        let deadline = Instant::now() + Duration::from_millis(20);
        assert!(matches!(
            mutex.lock_until(deadline),
            Err(LockError::TimedOut)
        ));
        assert!(Instant::now() >= deadline);
        drop(guard);
        *mutex
            .lock_until(Instant::now() + Duration::from_secs(2))
            .unwrap() = 1;
        assert_eq!(*mutex.lock().unwrap(), 1);
    }

    #[test]
    fn contended_unlocks_do_not_lose_notifications_or_updates() {
        let mutex = Arc::new(DeadlineMutex::new(0));
        let start = Arc::new(Barrier::new(8));
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let mutex = mutex.clone();
                let start = start.clone();
                scope.spawn(move || {
                    start.wait();
                    for _ in 0..100 {
                        let mut guard = mutex
                            .lock_until(Instant::now() + Duration::from_secs(2))
                            .unwrap();
                        *guard += 1;
                        std::thread::yield_now();
                    }
                });
            }
        });
        assert_eq!(*mutex.lock().unwrap(), 800);
    }

    #[test]
    fn panic_poison_is_reported_to_all_waiters() {
        let mutex = Arc::new(DeadlineMutex::new(0));
        let start = Arc::new(Barrier::new(5));
        std::thread::scope(|scope| {
            let holder = scope.spawn(|| {
                let _guard = mutex.lock().unwrap();
                start.wait();
                panic!("poison protected state");
            });
            let mut waiters = Vec::new();
            for _ in 0..4 {
                waiters.push(scope.spawn(|| {
                    start.wait();
                    matches!(
                        mutex.lock_until(Instant::now() + Duration::from_secs(2)),
                        Err(LockError::Poisoned)
                    )
                }));
            }
            assert!(holder.join().is_err());
            for waiter in waiters {
                assert!(waiter.join().unwrap());
            }
        });
    }
}
