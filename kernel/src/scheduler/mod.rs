//! Preemptive round-robin scheduling, one scheduler per core that runs
//! tasks.
//!
//! Core 0 runs the kernel's tasks (shell, filesystem, network, UI, ...);
//! the program cores (`PROGRAM_CORES`, 3 and 4 when QEMU has them) run
//! programs; cores 1 and 2 run thread and async work outside of any
//! scheduler (see `arch::smp`). Each new program goes to the program core
//! with the fewest tasks, and stays there. Without program cores, programs
//! run on core 0.
//!
//! Tasks are Ready or Blocked. The timer IRQ (and every other IRQ) calls
//! `schedule`, which picks the core's next Ready task; tasks give up the
//! CPU early with `yield_now`, or block on a `wait::WaitQueue` until
//! something wakes them. When nothing is Ready the core's idle task runs
//! `wfi`. Waking a task on another core also sends that core an interrupt
//! (`gic::kick`), so it doesn't wait for its next tick.
//!
//! Each core's state lives behind its own `IrqMutex`, and nothing ever
//! holds two of them at once. Task ids are unique across cores.
//!
//! Programs are tasks too, running in user mode (EL0) in their own address
//! space: each task carries the TTBR0 value to switch to. A program can be
//! killed, but only while it's in user mode, never halfway through a
//! syscall (it might hold a lock): `kill` marks it, and it's removed the
//! next time it's about to return to user mode.

pub mod asyncs;
pub mod task;
pub mod wait;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use crate::arch::irq;
use crate::arch::timer;
use crate::println;
use crate::std::lock::irq_mutex::IrqMutex;
use self::task::TrapFrame;

const STACK_SIZE: usize = 64 * 1024;
const IDLE_STACK_SIZE: usize = 16 * 1024;
pub const MAX_CORES: usize = 5;
/// The cores that run programs, if QEMU has them.
pub const PROGRAM_CORES: [usize; 2] = [3, 4];

pub type TaskId = u64;

/// What a syscall asked the scheduler to do, via `TrapFrame::request`.
#[repr(u64)]
pub enum Request {
    None = 0,
    Yield = 1,
    Exit = 2,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Ready,
    /// Waiting to be woken, or until the counter reaches `deadline`.
    Blocked { deadline: Option<u64> },
}

pub struct Task {
    id: TaskId,
    name: String,
    started_ms: u64,
    state: State,
    /// Timer ticks this task was running for.
    ticks: u64,
    frame: *mut TrapFrame,
    /// The address space it runs in, or 0 for the kernel's.
    ttbr0: u64,
    /// Asked to end; see `kill`.
    killed: bool,
    /// Its entry in `KOS_DEBUG_TASKS`, once it's been given to a scheduler.
    debug_slot: Option<usize>,
    _stack: Vec<u8>,
}

impl Drop for Task {
    fn drop(&mut self) {
        if let Some(slot) = self.debug_slot {
            KOS_DEBUG_TASKS[slot].id.store(0, Ordering::Release);
        }
    }
}

impl Task {
    fn new(id: TaskId, name: &str, initial: TrapFrame, stack_size: usize, ttbr0: u64) -> Self {
        let mut stack = vec![0u8; stack_size];
        let stack_top = stack.as_mut_ptr() as usize + stack_size;
        let frame = ((stack_top - size_of::<TrapFrame>()) & !0xf) as *mut TrapFrame;
        unsafe { frame.write(initial) };
        Task {
            id, name: String::from(name), started_ms: timer::uptime_ms(), state: State::Ready, ticks: 0, frame, ttbr0,
            killed: false, debug_slot: None, _stack: stack,
        }
    }

    fn kernel(id: TaskId, name: &str, entry: extern "C" fn() -> !, stack_size: usize) -> Self {
        Self::new(id, name, TrapFrame::new(entry as usize, task_returned as usize), stack_size, 0)
    }

    /// Killed, and stopped in user mode, so it can be removed safely.
    fn ready_to_reap(&self) -> bool {
        self.killed && unsafe { (*self.frame).is_user() }
    }
}

struct Scheduler {
    tasks: Vec<Task>,
    /// Index into `tasks` of the running task, unless `on_idle`.
    current: usize,
    idle: Option<Task>,
    on_idle: bool,
    /// Tasks that exited. Their stacks are freed on the next switch, once
    /// we're no longer running on any of them.
    dead: Vec<Task>,
    /// What TTBR0 holds right now.
    ttbr0: u64,
}

unsafe impl Send for Scheduler {}

static SCHEDULERS: [IrqMutex<Scheduler>; MAX_CORES] = [const { IrqMutex::new(Scheduler {
    tasks: Vec::new(),
    current: 0,
    idle: None,
    on_idle: false,
    dead: Vec::new(),
    ttbr0: 0,
}) }; MAX_CORES];
/// Which cores' schedulers are running.
static RUNNING: [AtomicBool; MAX_CORES] = [const { AtomicBool::new(false) }; MAX_CORES];
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
/// Per core: timer ticks that landed while the idle task was running, and
/// in total.
static IDLE_TICKS: [AtomicU64; MAX_CORES] = [const { AtomicU64::new(0) }; MAX_CORES];
static ALL_TICKS: [AtomicU64; MAX_CORES] = [const { AtomicU64::new(0) }; MAX_CORES];

// ---- for debuggers -------------------------------------------------------------
// lldb can't easily walk Rust's Vecs and Strings, so the scheduler also keeps
// a plain table of its tasks at a fixed symbol, for tools/lldb/kos.py: each
// task's id, core, name, address space, and where its registers were saved
// when it last stopped running. With that, a debugger can list every task
// and walk the stack of one that's blocked, not just the ones running.

const DEBUG_SLOTS: usize = 512;
const DEBUG_NAME_LEN: usize = 24;

/// One task, as tools/lldb/kos.py reads it: seven little-endian u64s.
#[repr(C)]
pub struct DebugTask {
    /// 0 for a free slot.
    id: AtomicU64,
    core: AtomicU64,
    /// The TrapFrame its registers were saved in when it last stopped.
    frame: AtomicU64,
    /// Its address space, or 0 for the kernel's.
    ttbr0: AtomicU64,
    /// The name, NUL-padded, 8 bytes to a word.
    name: [AtomicU64; DEBUG_NAME_LEN / 8],
}

#[unsafe(no_mangle)]
pub static KOS_DEBUG_TASKS: [DebugTask; DEBUG_SLOTS] = [const { DebugTask {
    id: AtomicU64::new(0),
    core: AtomicU64::new(0),
    frame: AtomicU64::new(0),
    ttbr0: AtomicU64::new(0),
    name: [const { AtomicU64::new(0) }; DEBUG_NAME_LEN / 8],
} }; DEBUG_SLOTS];

/// The task each core is running, or 0 while it's idle.
#[unsafe(no_mangle)]
pub static KOS_DEBUG_CURRENT: [AtomicU64; MAX_CORES] = [const { AtomicU64::new(0) }; MAX_CORES];

/// Puts `task` in a free slot of the debugger table (if there is one).
fn debug_register(task: &mut Task, core: usize) {
    let Some(slot) = KOS_DEBUG_TASKS.iter().position(|t| {
        t.id.compare_exchange(0, task.id, Ordering::AcqRel, Ordering::Relaxed).is_ok()
    }) else { return };
    let entry = &KOS_DEBUG_TASKS[slot];
    entry.core.store(core as u64, Ordering::Relaxed);
    entry.frame.store(task.frame as u64, Ordering::Relaxed);
    entry.ttbr0.store(task.ttbr0, Ordering::Relaxed);
    let mut name = [0u8; DEBUG_NAME_LEN];
    let n = task.name.len().min(DEBUG_NAME_LEN - 1);
    name[..n].copy_from_slice(&task.name.as_bytes()[..n]);
    for (word, bytes) in entry.name.iter().zip(name.chunks(8)) {
        word.store(u64::from_le_bytes(bytes.try_into().unwrap()), Ordering::Relaxed);
    }
    task.debug_slot = Some(slot);
}

fn this_core() -> usize {
    crate::arch::psci::get_core_id() as usize
}

/// This core's scheduler. Only call where `can_yield()` is true.
fn here() -> &'static IrqMutex<Scheduler> {
    &SCHEDULERS[this_core()]
}

/// Whether `core`'s scheduler is running.
pub fn is_running(core: usize) -> bool {
    RUNNING.get(core).is_some_and(|r| r.load(Ordering::Acquire))
}

/// Gets `core` to look at its tasks now, if it isn't this core.
fn kick(core: usize) {
    if core != this_core() && is_running(core) {
        crate::arch::gic::kick(core);
    }
}

impl Scheduler {
    fn current_task(&mut self) -> &mut Task {
        if self.on_idle { self.idle.as_mut().unwrap() } else { &mut self.tasks[self.current] }
    }

    /// Wakes tasks whose deadline has passed.
    fn expire_deadlines(&mut self) {
        let now = timer::now();
        for task in &mut self.tasks {
            if let State::Blocked { deadline: Some(d) } = task.state {
                if now >= d {
                    task.state = State::Ready;
                    log::trace!("task {} '{}' woke up (deadline)", task.id, task.name);
                }
            }
        }
    }

    /// Switches to the next Ready task after the current one (possibly the
    /// current one again), or to idle if none is Ready.
    fn switch_to_next(&mut self, start_after: Option<usize>) -> *mut TrapFrame {
        let before = if self.on_idle { 0 } else { self.tasks.get(self.current).map_or(0, |t| t.id) };
        let frame = self.pick_next(start_after);
        let ttbr0 = match self.current_task().ttbr0 {
            0 => crate::arch::mmu::kernel_ttbr0(),
            t => t,
        };
        if ttbr0 != self.ttbr0 {
            crate::arch::mmu::set_ttbr0(ttbr0);
            self.ttbr0 = ttbr0;
        }
        let after = if self.on_idle { 0 } else { self.tasks[self.current].id };
        KOS_DEBUG_CURRENT[this_core()].store(after, Ordering::Relaxed);
        if before != after {
            crate::stats::inc(&crate::stats::CONTEXT_SWITCHES);
            let name = if self.on_idle { "idle" } else { self.tasks[self.current].name.as_str() };
            log::trace!("switch {} -> {} '{}'", before, after, name);
        }
        frame
    }

    fn pick_next(&mut self, start_after: Option<usize>) -> *mut TrapFrame {
        let n = self.tasks.len();
        let first = start_after.map_or(0, |i| i + 1);
        for step in 0..n {
            let i = (first + step) % n;
            if self.tasks[i].state == State::Ready {
                self.current = i;
                self.on_idle = false;
                return self.tasks[i].frame;
            }
        }
        self.on_idle = true;
        self.idle.as_ref().unwrap().frame
    }
}

/// Where a task's entry function "returns" to: exit it cleanly.
extern "C" fn task_returned() -> ! {
    unsafe { asm!("svc #0", options(noreturn)) }
}

extern "C" fn idle_task() -> ! {
    loop {
        unsafe { asm!("wfi") };
    }
}

fn add_task(core: usize, mut task: Task) {
    debug_register(&mut task, core);
    SCHEDULERS[core].lock().tasks.push(task);
    kick(core);
}

/// Starts a kernel task. Kernel tasks run on core 0.
pub fn spawn(name: &str, entry: extern "C" fn() -> !) -> TaskId {
    spawn_with_stack(name, entry, STACK_SIZE)
}

/// Like `spawn`, for tasks that need more than the usual 64 KiB of stack.
/// There's no guard page, so running off the end corrupts the heap.
pub fn spawn_with_stack(name: &str, entry: extern "C" fn() -> !, stack_size: usize) -> TaskId {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    // Allocated before taking the scheduler's lock.
    add_task(0, Task::kernel(id, name, entry, stack_size));
    crate::stats::inc(&crate::stats::TASKS_SPAWNED);
    log::info!("spawned task {} '{}' (entry {:#x})", id, name, entry as usize);
    id
}

/// The program core with the fewest tasks, or core 0 if none is running.
fn core_for_program() -> usize {
    PROGRAM_CORES.iter().copied()
        .filter(|&c| is_running(c))
        .min_by_key(|&c| SCHEDULERS[c].lock().tasks.len())
        .unwrap_or(0)
}

/// Starts a program: a task running `entry` in user mode on the user stack
/// `sp`, in the address space `ttbr0`; returning from `entry` jumps to
/// `ret`. Syscalls and interrupts run on its kernel stack. It goes to a
/// program core.
pub fn spawn_user(name: &str, entry: usize, sp: usize, ret: usize, ttbr0: u64) -> TaskId {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let task = Task::new(id, name, TrapFrame::new_user(entry, sp, ret), STACK_SIZE, ttbr0);
    let core = core_for_program();
    add_task(core, task);
    crate::stats::inc(&crate::stats::TASKS_SPAWNED);
    log::info!("spawned program {} '{}' on core {} (entry {:#x})", id, name, core, entry);
    id
}

/// Called from IRQ context (IRQs masked): saves `current` and returns the
/// next task's frame.
pub fn schedule(current: *mut TrapFrame) -> *mut TrapFrame {
    if !can_yield() {
        return current;
    }
    let next = {
        let mut sched = here().lock();
        sched.dead.clear();
        let task = sched.current_task();
        task.frame = current;
        if let Some(slot) = task.debug_slot {
            KOS_DEBUG_TASKS[slot].frame.store(current as u64, Ordering::Relaxed);
        }
        sched.expire_deadlines();
        let after = if sched.on_idle { None } else { Some(sched.current) };
        sched.switch_to_next(after)
    };
    reap_killed(next)
}

/// While the task about to run is a killed program, removes it instead.
fn reap_killed(mut next: *mut TrapFrame) -> *mut TrapFrame {
    loop {
        let reap = {
            let mut sched = here().lock();
            !sched.on_idle && sched.current_task().ready_to_reap()
        };
        if !reap {
            return next;
        }
        next = remove_current();
    }
}

/// Removes the current task and returns the next one to run. IRQs masked.
pub fn exit_current(_current: *mut TrapFrame) -> *mut TrapFrame {
    let next = remove_current();
    reap_killed(next)
}

fn remove_current() -> *mut TrapFrame {
    // Let the process table know first, without the scheduler locked: it
    // wakes whoever is waiting for the program to finish.
    let id = here().lock().current_task().id;
    crate::process::exited(id);
    let mut sched = here().lock();
    let index = sched.current;
    let dead = sched.tasks.remove(index);
    crate::stats::inc(&crate::stats::TASKS_EXITED);
    log::info!("task {} '{}' {} after {} ms", dead.id, dead.name, if dead.killed { "killed" } else { "exited" },
        timer::uptime_ms() - dead.started_ms);
    sched.dead.push(dead);
    // The task now at `index` (if any) is the one after the dead one.
    let after = index.checked_sub(1).or(sched.tasks.len().checked_sub(1));
    sched.switch_to_next(after)
}

/// Acts on `frame.request` once a syscall has finished, and ends a killed
/// program before it gets back to user mode. IRQs masked.
pub fn after_syscall(frame: &mut TrapFrame) -> *mut TrapFrame {
    let request = core::mem::replace(&mut frame.request, Request::None as u64);
    if frame.is_user() && current_killed() {
        return exit_current(frame);
    }
    match request {
        r if r == Request::Yield as u64 => schedule(frame),
        r if r == Request::Exit as u64 => exit_current(frame),
        _ => frame,
    }
}

/// Runs `f` on task `id`, wherever it is, and kicks its core. Returns
/// whether the task exists.
fn with_task(id: TaskId, f: impl Fn(&mut Task)) -> bool {
    for core in 0..MAX_CORES {
        let found = {
            let mut sched = SCHEDULERS[core].lock();
            match sched.tasks.iter_mut().find(|t| t.id == id) {
                Some(task) => {
                    f(task);
                    true
                }
                None => false,
            }
        };
        if found {
            kick(core);
            return true;
        }
    }
    false
}

/// Asks task `id` to end: it's removed the next time it would run in user
/// mode. A task blocked in a syscall is woken, and long waits (reading a
/// line, sleeping) give up early for killed tasks. Only use this on
/// programs: kernel tasks never return to user mode. Returns whether the
/// task exists.
pub fn kill(id: TaskId) -> bool {
    with_task(id, |task| {
        task.killed = true;
        task.state = State::Ready;
        log::debug!("task {} '{}' marked to be killed", task.id, task.name);
    })
}

/// Whether the running task has been killed, so waits should give up.
pub fn current_killed() -> bool {
    if !can_yield() {
        return false;
    }
    let mut sched = here().lock();
    !sched.on_idle && sched.current_task().killed
}

/// For log lines: copies the running task's name into `name` and returns
/// its id, or None on a core without a scheduler / if the scheduler is
/// busy (it never waits, so it's safe from anywhere).
pub fn current_task_label(name: &mut [u8; 12]) -> Option<TaskId> {
    if !can_yield() {
        return None;
    }
    let mut sched = here().try_lock()?;
    let task = sched.current_task();
    let n = task.name.len().min(name.len());
    name[..n].copy_from_slice(&task.name.as_bytes()[..n]);
    Some(task.id)
}

/// The running task's id. Only meaningful where `can_yield()` is true.
pub fn current_id() -> TaskId {
    let mut sched = here().lock();
    sched.current_task().id
}

/// Marks the running task Blocked; it stops being scheduled once it yields
/// (or is preempted). See `wait::WaitQueue` for the race-free way to use this.
pub fn block_current(deadline: Option<u64>) {
    let mut sched = here().lock();
    if !sched.on_idle {
        let task = sched.current_task();
        task.state = State::Blocked { deadline };
        log::trace!("task {} '{}' blocked{}", task.id, task.name, if deadline.is_some() { " with deadline" } else { "" });
    }
}

/// Makes task `id` Ready if it's Blocked. Safe from any context, on any core.
pub fn wake(id: TaskId) {
    with_task(id, |task| {
        if task.state != State::Ready {
            log::trace!("task {} '{}' woken", task.id, task.name);
        }
        task.state = State::Ready;
    });
}

/// Called by the timer tick, on every core with a scheduler, for CPU
/// accounting.
pub fn account_tick() {
    if !can_yield() {
        return;
    }
    let core = this_core();
    ALL_TICKS[core].fetch_add(1, Ordering::Relaxed);
    let mut sched = here().lock();
    if sched.on_idle {
        IDLE_TICKS[core].fetch_add(1, Ordering::Relaxed);
    } else {
        sched.current_task().ticks += 1;
    }
}

/// Share of `core`'s time spent idle since boot, in percent.
pub fn idle_percent_of(core: usize) -> u64 {
    IDLE_TICKS[core].load(Ordering::Relaxed) * 100 / ALL_TICKS[core].load(Ordering::Relaxed).max(1)
}

/// Share of core 0's time spent idle since boot, in percent.
pub fn idle_percent() -> u64 {
    idle_percent_of(0)
}

/// The cores running a scheduler.
pub fn scheduler_cores() -> Vec<usize> {
    (0..MAX_CORES).filter(|&c| is_running(c)).collect()
}

/// A snapshot of one task, for `ps`, the Task Manager and K-AI.
pub struct TaskInfo {
    pub id: TaskId,
    pub name: String,
    /// The core it runs on.
    pub core: usize,
    /// "running", "ready", "sleeping" or "blocked".
    pub state: &'static str,
    /// Share of its core's time since boot spent running this task, percent.
    pub cpu_percent: u64,
    /// Timer ticks it has run for; see `all_ticks`.
    pub ticks: u64,
}

/// Timer ticks on `core` since boot, running anything or idle.
pub fn all_ticks(core: usize) -> u64 {
    ALL_TICKS[core].load(Ordering::Relaxed)
}

/// Every task, core by core, each core's in the order they were started.
pub fn tasks() -> Vec<TaskInfo> {
    let mut all = Vec::new();
    for core in 0..MAX_CORES {
        let total = ALL_TICKS[core].load(Ordering::Relaxed).max(1);
        let sched = SCHEDULERS[core].lock();
        all.extend(sched.tasks.iter().enumerate().map(|(i, t)| {
            let running = is_running(core) && !sched.on_idle && i == sched.current;
            let state = match t.state {
                _ if running => "running",
                State::Ready => "ready",
                State::Blocked { deadline: Some(_) } => "sleeping",
                State::Blocked { deadline: None } => "blocked",
            };
            TaskInfo { id: t.id, name: t.name.clone(), core, state, cpu_percent: t.ticks * 100 / total, ticks: t.ticks }
        }));
    }
    all
}

/// Every task, for `ps` (the `tasks` syscall): a tab-separated line each,
/// "id core state cpu kind name" (kind is "kernel", or "program" and how
/// it's connected to the console), then "core" lines with each scheduler
/// core and how idle it's been since boot, in percent.
pub fn tasks_text() -> String {
    use core::fmt::Write;
    let programs = crate::process::list();
    let mut text = String::new();
    for t in tasks() {
        let kind = match programs.iter().find(|p| p.task == t.id) {
            Some(p) => alloc::format!("program {}", p.console.name()),
            None => String::from("kernel"),
        };
        let _ = writeln!(text, "{}\t{}\t{}\t{}\t{}\t{}", t.id, t.core, t.state, t.cpu_percent, kind, t.name);
    }
    for core in scheduler_cores() {
        let _ = writeln!(text, "core\t{}\t{}", core, idle_percent_of(core));
    }
    text
}

/// Whether a fault came from a program in user mode (which can be killed)
/// rather than from the kernel itself.
pub fn is_user_fault(frame: &TrapFrame) -> bool {
    can_yield() && frame.is_user()
}

/// True on a core whose scheduler is running, i.e. where `yield_now`
/// would actually switch tasks and blocking is possible.
pub fn can_yield() -> bool {
    is_running(this_core())
}

/// Gives the rest of this time slice to the next task. A no-op anywhere
/// the scheduler doesn't run (thread and async cores, before boot finishes).
pub fn yield_now() {
    if can_yield() {
        unsafe { asm!("svc #{n}", n = const crate::syscall::SYS_YIELD) };
    }
}

/// For wait loops: yield where there's a scheduler, otherwise wait for an
/// event.
pub fn relax() {
    if can_yield() {
        yield_now();
    } else {
        unsafe { asm!("wfe", options(nomem, nostack)) };
    }
}

unsafe extern "C" {
    fn start_first_task(frame: *const TrapFrame) -> !;
}

/// Starts this core's scheduler; never returns. Core 0 starts first; the
/// program cores wait for it (`arch::smp`).
pub fn run() -> ! {
    irq::disable();
    let core = this_core();
    let frame = {
        let mut sched = SCHEDULERS[core].lock();
        sched.idle = Some(Task::kernel(0, "idle", idle_task, IDLE_STACK_SIZE));
        sched.switch_to_next(None)
    };
    RUNNING[core].store(true, Ordering::SeqCst);
    // Program cores waiting for core 0 to get here.
    unsafe { asm!("sev") };
    if core != 0 {
        log::info!("core {} is running programs", core);
    }
    unsafe { start_first_task(frame) }
}

pub type TaskSignature = extern "C" fn() -> !;
