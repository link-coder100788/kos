//! Blocking until something happens.
//!
//! A task waits on a `WaitQueue` for a condition; whoever makes the
//! condition true calls `wake_all`. Wakeups can be spurious (the condition
//! is always re-checked), but are never lost: the waiter marks itself
//! Blocked *before* its final check, so a wake that lands in between just
//! makes it Ready again.
//!
//! The condition runs with IRQs masked, so it must only touch lock-free
//! state or `IrqMutex`es — never a plain `Mutex`, whose holder might be a
//! preempted task that can't run until we unmask.

use alloc::vec::Vec;
use crate::arch::{irq, timer};
use crate::scheduler::{self, TaskId};
use crate::std::lock::irq_mutex::IrqMutex;

pub struct WaitQueue {
    waiters: IrqMutex<Vec<TaskId>>,
}

impl WaitQueue {
    pub const fn new() -> Self {
        Self { waiters: IrqMutex::new(Vec::new()) }
    }

    /// Blocks until `ready()` returns true. Off core 0 (or before the
    /// scheduler runs) this spins instead.
    pub fn wait_until(&self, ready: impl FnMut() -> bool) {
        self.wait_until_deadline(None, ready);
    }

    /// Like `wait_until`, but gives up when the timer counter reaches
    /// `deadline` (see `timer::deadline_after_ms`). Returns whether
    /// `ready()` became true.
    pub fn wait_until_deadline(&self, deadline: Option<u64>, mut ready: impl FnMut() -> bool) -> bool {
        loop {
            if ready() {
                return true;
            }
            if deadline.is_some_and(|d| timer::now() >= d) {
                return false;
            }
            if !scheduler::can_yield() {
                core::hint::spin_loop();
                continue;
            }
            let became_ready = irq::without_irqs(|| {
                scheduler::block_current(deadline);
                let id = scheduler::current_id();
                {
                    let mut waiters = self.waiters.lock();
                    if !waiters.contains(&id) {
                        waiters.push(id);
                    }
                }
                let ready_now = ready();
                if ready_now {
                    scheduler::wake(id);
                }
                ready_now
            });
            if became_ready {
                return true;
            }
            scheduler::yield_now();
        }
    }

    /// Wakes every waiting task. Safe from any context, including IRQs.
    pub fn wake_all(&self) {
        let waiters = core::mem::take(&mut *self.waiters.lock());
        for id in waiters {
            scheduler::wake(id);
        }
    }
}

static SLEEPERS: WaitQueue = WaitQueue::new();

/// Blocks the calling task for at least `ms` milliseconds (rounded up to
/// the 10 ms scheduler tick).
pub fn sleep_ms(ms: u64) {
    // A killed program stops sleeping, so it can be removed.
    SLEEPERS.wait_until_deadline(Some(timer::deadline_after_ms(ms)), scheduler::current_killed);
}
