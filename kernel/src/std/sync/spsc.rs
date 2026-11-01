//! Lock-free single-producer/single-consumer ring buffer. Safe under
//! arbitrary preemption of either side: there is no critical section, just
//! head/tail indices that only move forward.

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicUsize, Ordering};

pub struct SpscQueue<T, const N: usize> {
    buf: UnsafeCell<[MaybeUninit<T>; N]>,
    head: AtomicUsize, // next slot the consumer will read
    tail: AtomicUsize, // next slot the producer will write
}

// Safety: access is only ever from one producer and one consumer, coordinated
// by the head/tail atomics below. T must be Send to cross that boundary.
unsafe impl<T: Send, const N: usize> Sync for SpscQueue<T, N> {}

impl<T, const N: usize> SpscQueue<T, N> {
    pub const fn new() -> Self {
        Self {
            // Safety: an array of MaybeUninit<T> needs no initialization.
            buf: UnsafeCell::new(unsafe { MaybeUninit::uninit().assume_init() }),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Single-producer only. Returns the value back in Err if full.
    pub fn push(&self, val: T) -> Result<(), T> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        if tail.wrapping_sub(head) >= N {
            return Err(val);
        }
        let idx = tail % N;
        unsafe {
            (*self.buf.get())[idx].write(val);
        }
        // Release: publish the write above before advancing tail.
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        Ok(())
    }

    /// Safe to call from either side; the answer may be stale by the time
    /// the caller acts on it.
    pub fn is_empty(&self) -> bool {
        self.head.load(Ordering::Acquire) == self.tail.load(Ordering::Acquire)
    }

    /// Single-consumer only.
    pub fn pop(&self) -> Option<T> {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        if head == tail {
            return None;
        }
        let idx = head % N;
        // Safety: idx was written by push and is not yet claimed by a
        // previous pop, since head hasn't advanced past it.
        let val = unsafe { (*self.buf.get())[idx].assume_init_read() };
        self.head.store(head.wrapping_add(1), Ordering::Release);
        Some(val)
    }
}
