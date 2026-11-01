use core::{cell::UnsafeCell, sync::atomic::AtomicU32};
use core::ops::Deref;
use core::ops::Drop;
use core::arch::asm;
use core::sync::atomic::Ordering;
use core::ops::DerefMut;

pub const WRITE_LOCKED: u32 = 1 << 31;
pub const READER_MASK: u32 = !WRITE_LOCKED;

pub struct RwLock<T> {
    state: AtomicU32,
    value: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for RwLock<T> {}
unsafe impl<T: Send + Sync> Sync for RwLock<T> {}

impl<T> RwLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            state: AtomicU32::new(0),
            value: UnsafeCell::new(value),
        }
    }

    pub fn read(&self) -> RwLockReadGuard<'_, T> {
        loop {
            let state = self.state.load(Ordering::Relaxed);
            if state & WRITE_LOCKED != 0 || state == READER_MASK {
                unsafe { asm!("wfe", options(nomem, nostack)) };
                continue;
            }
            if self.state.compare_exchange_weak(
                state,
                state + 1,
                Ordering::Acquire,
                Ordering::Relaxed,
            ).is_ok() {
                return RwLockReadGuard { lock: self };
            }
        }
    }

    pub fn write(&self) -> RwLockWriteGuard<'_, T> {
        loop {
            if self.state.compare_exchange_weak(
                0,
                WRITE_LOCKED,
                Ordering::Acquire,
                Ordering::Relaxed,
            ).is_ok() {
                return RwLockWriteGuard { lock: self };
            }
            while self.state.load(Ordering::Relaxed) != 0 {
                unsafe { asm!("wfe", options(nomem, nostack)) };
            }
        }
    }
}

pub struct RwLockReadGuard<'a, T> {
    lock: &'a RwLock<T>,
}

impl<'a, T> Deref for RwLockReadGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.value.get() }
    }
}

impl<'a, T> Drop for RwLockReadGuard<'a, T> {
    fn drop(&mut self) {
        let previous_state = self.lock.state.fetch_sub(1, Ordering::Release);
        if previous_state == 1 {
            unsafe { asm!("sev", options(nomem, nostack)) };
        }
    }
}

pub struct RwLockWriteGuard<'a, T> {
    lock: &'a RwLock<T>,
}

impl<'a, T> Deref for RwLockWriteGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.value.get() }
    }
}

impl<'a, T> DerefMut for RwLockWriteGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<'a, T> Drop for RwLockWriteGuard<'a, T> {
    fn drop(&mut self) {
        self.lock.state.store(0, Ordering::Release);
        unsafe { asm!("sev", options(nomem, nostack)) };
    }
}