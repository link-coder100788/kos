//! What the secondary cores do. The bootloader sends cores 1-2 to the stub
//! that ends in `secondary_core_entry` and cores 3-4 to the one that ends
//! in `async_core_entry` (see arch/boot.s); both lead to `core_main`,
//! which gives each core its job:
//!
//! - core 1 runs `std::thread` tasks
//! - core 2 runs async tasks
//! - cores 3 and 4 run programs, each with its own scheduler
//!
//! If QEMU has fewer cores, core 0 picks up the missing jobs.

use core::arch::asm;
use core::task::{Context, Poll, Waker};
use crate::id_println;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use crate::scheduler::wait::WaitQueue;
use crate::scheduler::asyncs::ASYNC_TASK_QUEUE;

/// Set when work is queued, so the core-0 fallback workers can sleep
/// until there's something to do.
static THREAD_WORK: AtomicBool = AtomicBool::new(false);
static ASYNC_WORK: AtomicBool = AtomicBool::new(false);
static THREAD_WAIT: WaitQueue = WaitQueue::new();
static ASYNC_WAIT: WaitQueue = WaitQueue::new();

/// Called after queueing a std::thread task: wakes whoever runs them.
pub fn notify_thread_work() {
    THREAD_WORK.store(true, Ordering::Release);
    THREAD_WAIT.wake_all();
    unsafe { asm!("sev") };
}

/// Called after queueing an async task: wakes whoever runs them.
pub fn notify_async_work() {
    ASYNC_WORK.store(true, Ordering::Release);
    ASYNC_WAIT.wake_all();
    unsafe { asm!("sev") };
}

/// Bit n set = core n has reached its entry loop.
static ONLINE: AtomicU64 = AtomicU64::new(1);

pub fn is_online(core: u64) -> bool {
    ONLINE.load(Ordering::Acquire) & (1 << core) != 0
}

fn mark_online() {
    ONLINE.fetch_or(1 << crate::arch::psci::get_core_id(), Ordering::Release);
}

/// Runs one queued `std::thread` task, if any. Returns whether it did.
fn run_thread_task() -> bool {
    use crate::std::thread::TASK_QUEUE;
    let task = {
        let guard = TASK_QUEUE.lock();
        let mut queue = guard.borrow_mut();
        if queue.is_empty() { None } else { Some(queue.remove(0)) }
    };
    match task {
        Some(task) => { (task.run)(); true }
        None => false,
    }
}

/// Polls one queued async task, if any. Returns whether it did.
fn run_async_task() -> bool {
    let task = {
        let mut queue = ASYNC_TASK_QUEUE.lock();
        if queue.is_empty() { None } else { Some(queue.remove(0)) }
    };
    let Some(task) = task else { return false };
    let mut future_slot = task.future.lock();
    if let Some(mut future) = future_slot.take() {
        let waker: Waker = Waker::from(task.clone());
        let mut context = Context::from_waker(&waker);
        match future.as_mut().poll(&mut context) {
            Poll::Ready(()) => {
                log::debug!("async task {} finished", task.task_id);
            }
            Poll::Pending => {
                *future_slot = Some(future);
            }
        }
    }
    true
}

/// A secondary core's job.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Threads,
    Async,
    Programs,
}

pub fn role(core: u64) -> Role {
    match core {
        1 => Role::Threads,
        2 => Role::Async,
        _ => Role::Programs,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn secondary_core_entry() -> ! {
    core_main()
}

#[unsafe(no_mangle)]
pub extern "C" fn async_core_entry() -> ! {
    core_main()
}

fn core_main() -> ! {
    crate::arch::mmu::enable_on_this_core();
    let core = crate::arch::psci::get_core_id();
    match role(core) {
        Role::Threads => {
            mark_online();
            log::debug!("core {}: thread worker", core);
            loop {
                if !run_thread_task() {
                    unsafe { asm!("wfe", options(nomem, nostack)); }
                }
            }
        }
        Role::Async => {
            mark_online();
            log::debug!("core {}: async executor", core);
            loop {
                if !run_async_task() {
                    unsafe { asm!("wfe", options(nomem, nostack)); }
                }
            }
        }
        Role::Programs => {
            crate::arch::exceptions::init();
            crate::arch::gic::init_this_core();
            mark_online();
            // Tasks allocate, and spawning needs core 0's scheduler, so
            // wait for core 0 to start its own.
            while !crate::scheduler::is_running(0) {
                unsafe { asm!("wfe", options(nomem, nostack)); }
            }
            crate::arch::timer::init_this_core();
            crate::scheduler::run()
        }
    }
}

/// Core-0 stand-in for the thread cores, spawned when none came up.
pub extern "C" fn thread_worker_task() -> ! {
    loop {
        while run_thread_task() {}
        THREAD_WAIT.wait_until(|| THREAD_WORK.swap(false, Ordering::AcqRel));
    }
}

/// Core-0 stand-in for the async cores, spawned when none came up.
pub extern "C" fn async_worker_task() -> ! {
    loop {
        while run_async_task() {}
        ASYNC_WAIT.wait_until(|| ASYNC_WORK.swap(false, Ordering::AcqRel));
    }
}
