//! A spinlock that masks IRQs on the current core while held.
//!
//! Use it for anything an interrupt handler touches, or anything checked
//! inside a `WaitQueue` condition (which runs with IRQs masked): a holder
//! can't be preempted, so nobody on its core can spin on it forever.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};
use core::arch::asm;

pub struct IrqMutex<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for IrqMutex<T> {}
unsafe impl<T: Send> Sync for IrqMutex<T> {}

impl<T> IrqMutex<T> {
    pub const fn new(value: T) -> Self {
        Self { locked: AtomicBool::new(false), value: UnsafeCell::new(value) }
    }

    pub fn lock(&self) -> IrqMutexGuard<'_, T> {
        let daif: u64;
        unsafe { asm!("mrs {0}, daif", "msr daifset, #2", out(reg) daif, options(nostack)) };
        while self.locked.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            core::hint::spin_loop();
        }
        IrqMutexGuard { mutex: self, daif }
    }

    /// Takes the lock only if it's free right now.
    pub fn try_lock(&self) -> Option<IrqMutexGuard<'_, T>> {
        let daif: u64;
        unsafe { asm!("mrs {0}, daif", "msr daifset, #2", out(reg) daif, options(nostack)) };
        if self.locked.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok() {
            Some(IrqMutexGuard { mutex: self, daif })
        } else {
            unsafe { asm!("msr daif, {0}", in(reg) daif, options(nostack)) };
            None
        }
    }
}

pub struct IrqMutexGuard<'a, T> {
    mutex: &'a IrqMutex<T>,
    daif: u64,
}

impl<T> Deref for IrqMutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { &*self.mutex.value.get() }
    }
}

impl<T> DerefMut for IrqMutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.mutex.value.get() }
    }
}

impl<T> Drop for IrqMutexGuard<'_, T> {
    fn drop(&mut self) {
        self.mutex.locked.store(false, Ordering::Release);
        unsafe { asm!("msr daif, {0}", in(reg) self.daif, options(nostack)) };
    }
}
