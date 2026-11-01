//! IPC between uart_read_task (producer/caller) and fs_task (consumer/server).
//!
//! Uses a lock-free single-producer/single-consumer ring buffer for both
//! directions. This is deliberately lock-free rather than spinlock-protected:
//! on a preemptive scheduler with no yield syscall yet, a spinlock held by a
//! task that gets preempted mid-critical-section would leave the other task
//! spinning until the scheduler happens to come back around to the lock
//! holder. A lock-free SPSC queue has no critical section that must complete
//! atomically — head/tail indices only ever move forward, and Acquire/Release
//! ordering ensures a written slot is visible before its index is published.
//! That makes it safe under arbitrary preemption of either side.
//!
//! IMPORTANT: these queues are SPSC. If you add a second shell/tty task later
//! that also wants to talk to the filesystem, do NOT just share these same
//! queues from two producers — that's a data race. Either give each caller
//! its own pair of queues (simplest), or upgrade to a real MPSC structure.

use crate::filesys::backend::{DirEntryInfo, FsError};
use alloc::string::String;
use alloc::vec::Vec;
pub use crate::std::sync::spsc::SpscQueue;
use crate::scheduler::wait::WaitQueue;
use core::sync::atomic::{AtomicBool, Ordering};

// ---- Request / response protocol ---------------------------------------

#[derive(Clone)]
pub enum FsRequest {
    List { cwd: String, path: String },
    ChangeDir { cwd: String, path: String },
    Read { cwd: String, path: String },
    Write { cwd: String, path: String, data: Vec<u8> },
    Mkdir { cwd: String, path: String },
    Create { cwd: String, path: String },
    Remove { cwd: String, path: String },
}

pub enum FsResponse {
    Listing(Vec<DirEntryInfo>),
    Data(Vec<u8>),
    /// New absolute cwd path, for ChangeDir.
    Path(String),
    Ok,
    Err(FsError),
}

pub static FS_REQUEST_QUEUE: SpscQueue<FsRequest, 4> = SpscQueue::new();
pub static FS_RESPONSE_QUEUE: SpscQueue<FsResponse, 4> = SpscQueue::new();

/// Woken when a request is queued (fs_task waits on it) and when a
/// response is queued (the caller waits on it).
pub static FS_REQUEST_WAIT: WaitQueue = WaitQueue::new();
pub static FS_RESPONSE_WAIT: WaitQueue = WaitQueue::new();

// FS_REQUEST_QUEUE/FS_RESPONSE_QUEUE are SPSC, but several tasks call
// fs_call(). FS_CALL_BUSY makes a whole round trip mutually exclusive, so
// from the queues' point of view there's only ever one producer/consumer
// pair at a time. Callers that find it taken sleep on FS_CALL_WAIT.
static FS_CALL_BUSY: AtomicBool = AtomicBool::new(false);
static FS_CALL_WAIT: WaitQueue = WaitQueue::new();

/// Blocking call: submit a request and sleep until the response arrives.
/// Safe to call from any number of tasks.
pub fn fs_call(req: FsRequest) -> FsResponse {
    FS_CALL_WAIT.wait_until(|| {
        FS_CALL_BUSY.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok()
    });
    // Only one call is ever in flight, so the queues always have room.
    if FS_REQUEST_QUEUE.push(req).is_err() {
        panic!("fs_call: request queue full");
    }
    FS_REQUEST_WAIT.wake_all();
    let mut resp = None;
    FS_RESPONSE_WAIT.wait_until(|| {
        resp = FS_RESPONSE_QUEUE.pop();
        resp.is_some()
    });
    FS_CALL_BUSY.store(false, Ordering::Release);
    FS_CALL_WAIT.wake_all();
    resp.unwrap()
}
