//! Locks that keep working after a thread panicked while holding one.
//!
//! The std locks are "poisoned" by such a panic, and `.lock().unwrap()` then panics in every
//! later user too, the UI thread included. The data behind these locks (caches, connection
//! state, status lines) stays usable, so these take the guard anyway.

use std::sync::{Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

pub trait LockExt<T: ?Sized> {
    fn locked(&self) -> MutexGuard<'_, T>;
}

impl<T: ?Sized> LockExt<T> for Mutex<T> {
    fn locked(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

pub trait RwLockExt<T: ?Sized> {
    fn read_ok(&self) -> RwLockReadGuard<'_, T>;
    fn write_ok(&self) -> RwLockWriteGuard<'_, T>;
}

impl<T: ?Sized> RwLockExt<T> for RwLock<T> {
    fn read_ok(&self) -> RwLockReadGuard<'_, T> {
        self.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write_ok(&self) -> RwLockWriteGuard<'_, T> {
        self.write().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_poisoned_lock_still_opens() {
        let m = std::sync::Arc::new(Mutex::new(1));
        let r = std::sync::Arc::new(RwLock::new(1));
        let (m2, r2) = (m.clone(), r.clone());
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            let _w = r2.write().unwrap();
            panic!("poison");
        })
        .join();
        assert!(m.is_poisoned() && r.is_poisoned());
        *m.locked() += 1;
        *r.write_ok() += 1;
        assert_eq!((*m.locked(), *r.read_ok()), (2, 2));
    }
}
