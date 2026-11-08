//! Processes: programs loaded from disk, and what each one owns.
//!
//! Every program runs in user mode (EL0) in an address space of its own,
//! built from page frames as it needs them:
//!
//! ```text
//! EXIT_STUB                    one page holding `mov x0, #0; svc #0`,
//!                              where `_start` returns to
//! USER_BASE ..                 the program image, then its heap, which
//!           .. HEAP_LIMIT      grows as the program allocates
//!           .. STACK_LIMIT     never mapped: a stack overflow faults
//! STACK_LIMIT .. STACK_TOP     the stack: up to 8 MiB, mapped as it's used
//! ```
//!
//! Memory is only mapped when it's used, and given back when it isn't:
//! - heap pages are mapped when something is allocated in them, and
//!   unmapped (their frames freed) once everything in them is freed;
//! - the stack starts at 16 KiB, and touching the page below it faults
//!   into `grow_stack`, which maps down to there and lets the program
//!   carry on.
//!
//! Every program sees itself at the same addresses (K programs, linked to
//! run at `USER_BASE`, included) and can't see the kernel or any other
//! program. How big a program and its heap can get is only limited by
//! free memory. Each address space has an ASID (1-255), so up to 255
//! programs can run at once, and switching between them needs no TLB
//! flush.
//!
//! `alloc` hands out heap memory inside the program's address space, with
//! the bookkeeping in the kernel, out of the program's reach.
//!
//! A new program is built as a `NewProcess`, outside the process table, so
//! loading a big image never runs with IRQs masked; `start` then moves it
//! into the table. When a program's task ends, however it ends (`exit`, a
//! fault, or being killed), the scheduler calls `exited`. That only marks
//! it, since it runs mid-switch with IRQs masked; the loader task then
//! `reap`s it: closes its windows and file descriptors and frees its memory
//! and page tables.
//!
//! Every program ends with an exit status, 0-255: what it passed to `exit`
//! (0 if `_start` returned), or 130 if Ctrl+C ended it, 137 if it was
//! killed and 139 if it faulted, as Unix shells show them. `wait` returns
//! it once the program has been reaped, so whatever it wrote to files is
//! on the disk by then.
//!
//! Each program has its own file descriptors (see `fd`): 0-2 are its
//! stdin, stdout and stderr, given to it by whoever started it.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use crate::arch::mmu::{self, AddressSpace, MapFlags};
use crate::mm::frame;
use crate::mm::layout::{phys_to_virt, PAGE_SIZE};
use crate::scheduler::{self, wait::WaitQueue, TaskId};
use crate::std::lock::irq_mutex::IrqMutex;
use crate::ui::window::WindowId;
use crate::fd::{self, Fd};

/// Where programs are loaded. K programs are linked to run here.
pub const USER_BASE: usize = 0x5000_0000;
/// A program whose `_start` returns lands here, on an exit syscall.
pub const EXIT_STUB: usize = USER_BASE - PAGE_SIZE;
pub const STACK_TOP: usize = 0x40_0000_0000;
/// How far the stack can grow.
pub const STACK_MAX: usize = 8 * 1024 * 1024;
pub const STACK_LIMIT: usize = STACK_TOP - STACK_MAX;
/// How much stack a program starts with.
pub const STACK_INITIAL: usize = 16 * 1024;
/// Unmapping more pages than this at once flushes the program's whole TLB
/// instead of page by page.
const FLUSH_ALL_PAGES: usize = 64;
/// The image and heap must end below this; the rest, up to the stack, is
/// never mapped.
pub const HEAP_LIMIT: usize = 0x30_0000_0000;
/// The heap's address range grows by at least this much at a time (pages
/// are only mapped as they're allocated).
const HEAP_GROW: usize = 64 * 1024;
/// ASIDs 1..=255 go to programs (0 is the kernel's).
pub const MAX_PROGRAMS: usize = 255;
/// `mov x0, #0`, then `svc #0`: returning from `_start` exits with 0.
const EXIT_CODE: [u32; 2] = [0xd280_0000, 0xd400_0001];
/// Exit statuses for programs that didn't call `exit`.
pub const STATUS_INTERRUPTED: i32 = 130;
pub const STATUS_KILLED: i32 = 137;
pub const STATUS_FAULTED: i32 = 139;
/// How many file descriptors a program can have open.
pub const MAX_FDS: usize = 64;
/// How many ended programs' exit statuses are kept for `wait`.
const STATUSES_KEPT: usize = 256;

pub type Pid = u64;

/// How a program is connected to the console.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Console {
    /// Started with `run`: the shell waits for it, and it gets the
    /// keyboard and the screen.
    Foreground,
    /// Started from the Programs window: its output shows in the Terminal,
    /// but it gets no input.
    Output,
    /// Started with `bg`: no input, and its output is thrown away.
    Detached,
}

impl Console {
    pub fn name(self) -> &'static str {
        match self {
            Console::Foreground => "fg",
            Console::Output => "window",
            Console::Detached => "bg",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WindowKind {
    /// Labels and buttons (`ui::window::open_window`).
    Widgets,
    Canvas,
}

// ---- a program's memory ------------------------------------------------------

/// An address space and the page frames mapped into it. Dropping it frees
/// both; nothing may be running in it by then.
struct Memory {
    space: AddressSpace,
    /// Every page mapped: virtual address -> frame.
    pages: BTreeMap<usize, usize>,
}

const fn page_down(va: usize) -> usize {
    va & !(PAGE_SIZE - 1)
}

const fn page_up(va: usize) -> usize {
    (va + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

impl Memory {
    fn new() -> Self {
        Self { space: AddressSpace::new(), pages: BTreeMap::new() }
    }

    fn page_at(&self, va: usize) -> Option<usize> {
        self.pages.get(&page_down(va)).copied()
    }

    /// Maps the physical frame `pa` at `va` (page-aligned), taking it over.
    fn map_frame(&mut self, va: usize, pa: usize, flags: MapFlags) {
        self.space.map(va, pa, PAGE_SIZE, flags);
        self.pages.insert(va, pa);
    }

    /// How many pages of `va..va+len` aren't mapped yet.
    fn missing_pages(&self, va: usize, len: usize) -> usize {
        (page_down(va)..va + len).step_by(PAGE_SIZE).filter(|&p| self.page_at(p).is_none()).count()
    }

    /// Maps the pages of `va..va+len` that aren't mapped yet onto `frames`
    /// (zeroed ones, one per missing page; see `missing_pages`).
    fn map_frames(&mut self, va: usize, len: usize, mut frames: Vec<usize>, flags: MapFlags) {
        for page in (page_down(va)..va + len).step_by(PAGE_SIZE) {
            if self.page_at(page).is_none() {
                let pa = frames.pop().expect("too few frames for the missing pages");
                self.map_frame(page, pa, flags);
            }
        }
        // Spares, if a page was mapped meanwhile.
        frames.into_iter().for_each(frame::free);
    }

    /// Unmaps every page in `start..end` (page-aligned) and frees its
    /// frame, once the TLB (tagged `asid`) has forgotten it.
    fn unmap_range(&mut self, start: usize, end: usize, asid: u16) {
        let pages: Vec<usize> = self.pages.range(start..end).map(|(&va, _)| va).collect();
        let flush_each = pages.len() <= FLUSH_ALL_PAGES;
        let mut freed = Vec::with_capacity(pages.len());
        for va in pages {
            self.space.unmap_page(va);
            freed.push(self.pages.remove(&va).unwrap());
            if flush_each {
                mmu::flush_page(asid, va);
            }
        }
        if !flush_each {
            mmu::flush_asid(asid);
        }
        freed.into_iter().for_each(frame::free);
    }

    /// Makes sure every page of `va..va+len` is mapped, mapping zeroed
    /// pages where none are. False if memory ran out.
    fn map_range(&mut self, va: usize, len: usize, flags: MapFlags) -> bool {
        let mut page = va & !(PAGE_SIZE - 1);
        while page < va + len {
            if self.page_at(page).is_none() {
                let Some(pa) = frame::alloc_zeroed() else { return false };
                self.map_frame(page, pa, flags);
            }
            page += PAGE_SIZE;
        }
        true
    }

    /// Whether all of `va..va+len` is mapped.
    fn is_mapped(&self, va: usize, len: usize) -> bool {
        let Some(end) = va.checked_add(len) else { return false };
        let mut page = va & !(PAGE_SIZE - 1);
        while page < end {
            if self.page_at(page).is_none() {
                return false;
            }
            page += PAGE_SIZE;
        }
        true
    }

    /// Copies `bytes` to `va` (already mapped) through the kernel's view of
    /// the frames, a page at a time, and makes it visible to instruction
    /// fetch, since it may be code.
    fn write(&mut self, va: usize, bytes: &[u8]) -> bool {
        let mut done = 0;
        while done < bytes.len() {
            let at = va + done;
            let Some(pa) = self.page_at(at) else { return false };
            let offset = at % PAGE_SIZE;
            let n = (PAGE_SIZE - offset).min(bytes.len() - done);
            let dst = phys_to_virt(pa) + offset;
            unsafe { core::ptr::copy_nonoverlapping(bytes[done..].as_ptr(), dst as *mut u8, n) };
            mmu::sync_icache(dst, n);
            done += n;
        }
        true
    }
}

impl Drop for Memory {
    fn drop(&mut self) {
        for &pa in self.pages.values() {
            frame::free(pa);
        }
        // `space` frees its page tables when it's dropped, after this.
    }
}

// ---- the program heap ----------------------------------------------------------

/// A first-fit heap over part of the program's address space. Only the
/// kernel sees the bookkeeping, so a program can't corrupt it.
struct UserHeap {
    /// Free ranges, start -> length, never touching each other.
    free: BTreeMap<usize, usize>,
    /// Allocations, start -> length.
    used: BTreeMap<usize, usize>,
    total: usize,
    in_use: usize,
}

impl UserHeap {
    const fn empty() -> Self {
        Self { free: BTreeMap::new(), used: BTreeMap::new(), total: 0, in_use: 0 }
    }

    fn new(start: usize, end: usize) -> Self {
        let mut heap = Self::empty();
        if end > start {
            heap.add(start, end - start);
        }
        heap
    }

    fn alloc(&mut self, size: usize) -> Option<usize> {
        let size = Self::rounded(size)?;
        let (&start, &len) = self.free.iter().find(|(_, len)| **len >= size)?;
        self.free.remove(&start);
        if len > size {
            self.free.insert(start + size, len - size);
        }
        self.used.insert(start, size);
        self.in_use += size;
        Some(start)
    }

    /// What `alloc(size)` really takes.
    fn rounded(size: usize) -> Option<usize> {
        Some(size.max(1).checked_add(15)? & !15)
    }

    fn free(&mut self, ptr: usize) -> bool {
        self.release(ptr).is_some()
    }

    /// Frees an allocation and returns the whole free range it's now part
    /// of, as (start, length).
    fn release(&mut self, ptr: usize) -> Option<(usize, usize)> {
        let len = self.used.remove(&ptr)?;
        self.in_use -= len;
        Some(self.insert_free(ptr, len))
    }

    /// Adds fresh memory at `start..start+len` (the heap grew).
    fn add(&mut self, start: usize, len: usize) {
        self.total += len;
        self.insert_free(start, len);
    }

    /// Marks `start..start+len` free, merging it with the free ranges on
    /// either side, and returns the merged range.
    fn insert_free(&mut self, start: usize, len: usize) -> (usize, usize) {
        let (mut start, mut len) = (start, len);
        if let Some(next_len) = self.free.remove(&(start + len)) {
            len += next_len;
        }
        if let Some((&prev, &prev_len)) = self.free.range(..start).next_back() {
            if prev + prev_len == start {
                self.free.remove(&prev);
                start = prev;
                len += prev_len;
            }
        }
        self.free.insert(start, len);
        (start, len)
    }

    /// The size of the free range at the very end of the heap (which ends
    /// at `end`), if there is one.
    fn free_at_end(&self, end: usize) -> usize {
        self.free.iter().next_back().filter(|(s, l)| **s + **l == end).map_or(0, |(_, l)| *l)
    }
}

// ---- processes ------------------------------------------------------------------

struct Process {
    pid: Pid,
    name: String,
    args: String,
    task: TaskId,
    /// Its task has ended; waiting to be reaped.
    exited: bool,
    console: Console,
    asid: u16,
    memory: Memory,
    heap: UserHeap,
    /// Where the heap's mapped memory ends; it grows from here.
    heap_end: usize,
    windows: Vec<(WindowId, WindowKind)>,
    /// The last HTTP response it asked for, until it asks for another.
    http: Option<HttpReply>,
    /// Why its last network or process call failed.
    last_error: String,
    /// Its exit status, once it's known.
    status: Option<i32>,
    /// Its file descriptors, by number.
    fds: Vec<Option<Fd>>,
}

/// An HTTP response kept for a program to copy out piece by piece.
pub struct HttpReply {
    pub body: Vec<u8>,
    /// "Name: value\n" lines.
    pub headers: String,
}

struct Table {
    processes: Vec<Process>,
    /// Reaped programs' exit statuses, the newest last.
    statuses: alloc::collections::VecDeque<(TaskId, i32)>,
    /// Which ASIDs are taken (bit n = ASID n; 0 is the kernel's).
    asids: [u64; 4],
    next_pid: Pid,
}

impl Table {
    fn take_asid(&mut self) -> Option<u16> {
        let asid = (1..=MAX_PROGRAMS).find(|&a| self.asids[a / 64] & (1 << (a % 64)) == 0)?;
        self.asids[asid / 64] |= 1 << (asid % 64);
        Some(asid as u16)
    }

    fn release_asid(&mut self, asid: u16) {
        let a = asid as usize;
        self.asids[a / 64] &= !(1 << (a % 64));
    }
}

static TABLE: IrqMutex<Table> = IrqMutex::new(Table { processes: Vec::new(), statuses: alloc::collections::VecDeque::new(), asids: [1, 0, 0, 0], next_pid: 1 });
/// Woken when a program exits, and when it's been reaped.
pub static EXITED: WaitQueue = WaitQueue::new();
/// Some program has exited and not been reaped yet.
static TO_REAP: AtomicBool = AtomicBool::new(false);
/// Ctrl+C killed the foreground program.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// A program being loaded: its address space, not yet in the table or
/// running. Dropping it frees everything.
pub struct NewProcess {
    name: String,
    /// What followed the program's name on the command line.
    args: String,
    console: Console,
    memory: Memory,
    /// Its stdin, stdout and stderr.
    stdio: [Fd; 3],
}

impl NewProcess {
    /// An empty address space with just the start of the stack and the
    /// exit stub. None if memory ran out.
    pub fn new(name: &str, console: Console) -> Option<Self> {
        let mut memory = Memory::new();
        if !memory.map_range(STACK_TOP - STACK_INITIAL, STACK_INITIAL, MapFlags::USER_RW)
            || !memory.map_range(EXIT_STUB, PAGE_SIZE, MapFlags::USER_RWX)
            || !memory.write(EXIT_STUB, &exit_code_bytes()) {
            return None;
        }
        // A program in the background has nowhere to print or read from.
        let stdio = if console == Console::Detached { [fd::null(), fd::null(), fd::null()] } else { [fd::console(), fd::console(), fd::console()] };
        Some(Self { name: String::from(name), args: String::new(), console, memory, stdio })
    }

    /// Gives the program its stdin, stdout and stderr.
    pub fn set_stdio(&mut self, stdio: [Fd; 3]) {
        self.stdio = stdio;
    }

    /// Gives the program its command-line arguments (`args` syscall).
    pub fn set_args(&mut self, args: &str) {
        self.args = String::from(args);
    }
}

fn exit_code_bytes() -> [u8; 8] {
    let mut bytes = [0; 8];
    bytes[..4].copy_from_slice(&EXIT_CODE[0].to_le_bytes());
    bytes[4..].copy_from_slice(&EXIT_CODE[1].to_le_bytes());
    bytes
}

impl crate::loader::ProgramMemory for NewProcess {
    fn map(&mut self, va: usize, len: usize) -> Result<(), &'static str> {
        let end = va.checked_add(len).ok_or("address out of range")?;
        if va < USER_BASE || end > HEAP_LIMIT {
            return Err("address out of range");
        }
        if self.memory.map_range(va, len, MapFlags::USER_RWX) { Ok(()) } else { Err("out of memory") }
    }

    fn write(&mut self, va: usize, bytes: &[u8]) -> Result<(), &'static str> {
        if self.memory.write(va, bytes) { Ok(()) } else { Err("writing to unmapped memory") }
    }
}

/// Starts a loaded program at `entry`; its image ends at `image_end`, and
/// its heap starts after that. Fails if 255 programs are already running.
pub fn start(new: NewProcess, entry: usize, image_end: usize) -> Result<TaskId, &'static str> {
    let heap_start = (image_end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let NewProcess { name, args, console, memory, stdio } = new;
    let fds = stdio.into_iter().map(Some).collect();
    let mut memory = Some(memory);
    // It's added to the table before the lock is released, so even if it
    // runs at once on a program core and exits, `exited` finds it.
    crate::arch::irq::without_irqs(|| {
        let mut table = TABLE.lock();
        let asid = table.take_asid().ok_or("too many programs are running")?;
        let memory = memory.take().unwrap();
        let ttbr0 = mmu::ttbr0_value(&memory.space, asid);
        let task = scheduler::spawn_user(&name, entry, STACK_TOP, EXIT_STUB, ttbr0);
        let pid = table.next_pid;
        table.next_pid += 1;
        table.processes.push(Process {
            pid,
            name,
            args,
            task,
            exited: false,
            console,
            asid,
            memory,
            heap: UserHeap::new(heap_start, heap_start),
            heap_end: heap_start,
            windows: Vec::new(),
            http: None,
            last_error: String::new(),
            status: None,
            fds,
        });
        Ok(task)
    })
    // If it failed, `memory` (still Some) is dropped here, outside the lock.
}

/// Called by the scheduler when task `id` ends, mid-switch with IRQs
/// masked: if it was a program, marks it for `reap`.
pub fn exited(id: TaskId) {
    let found = {
        let mut table = TABLE.lock();
        match table.processes.iter_mut().find(|p| p.task == id) {
            Some(p) => {
                p.exited = true;
                true
            }
            None => false,
        }
    };
    if found {
        TO_REAP.store(true, Ordering::Release);
        EXITED.wake_all();
    }
}

/// Whether some exited program is waiting for `reap`.
pub fn needs_reaping() -> bool {
    TO_REAP.load(Ordering::Acquire)
}

/// Cleans up after every exited program: closes its windows and frees its
/// memory and page tables. Runs on a kernel task, so TTBR0 no longer
/// points at any of them.
pub fn reap() {
    TO_REAP.store(false, Ordering::Release);
    loop {
        let mut process = {
            let mut table = TABLE.lock();
            let Some(i) = table.processes.iter().position(|p| p.exited) else { return };
            table.processes.swap_remove(i)
        };
        // Closing its files writes them back, which waits for the disk.
        process.fds.clear();
        for &(id, kind) in &process.windows {
            match kind {
                WindowKind::Widgets => crate::ui::window::close_window(id),
                WindowKind::Canvas => crate::ui::apps::canvas::close(id),
            }
        }
        crate::net::close_owned_by(process.task);
        crate::sound::task_ended(process.task);
        crate::drivers::virtio::kgl::task_ended(process.task);
        log::debug!("reaped '{}' (pid {}): {} KiB of memory, {} windows", process.name, process.pid,
            process.memory.pages.len() * PAGE_SIZE / 1024, process.windows.len());
        mmu::flush_asid(process.asid);
        let asid = process.asid;
        let (task, status) = (process.task, process.status.unwrap_or(STATUS_KILLED));
        // Frees its pages and page tables, outside the lock.
        drop(process);
        {
            let mut table = TABLE.lock();
            table.release_asid(asid);
            if table.statuses.len() == STATUSES_KEPT {
                table.statuses.pop_front();
            }
            table.statuses.push_back((task, status));
        }
        EXITED.wake_all();
    }
}

/// Runs `f` on the calling program's process, or returns None if the
/// caller isn't a program.
fn with_current<R>(f: impl FnOnce(&mut Process) -> R) -> Option<R> {
    let id = scheduler::current_id();
    let mut table = TABLE.lock();
    table.processes.iter_mut().find(|p| p.task == id).map(f)
}

/// Zeroed frames, or None (with none taken) if memory ran out. Zeroing
/// takes a while, so callers get them before taking the table's lock.
fn zeroed_frames(count: usize) -> Option<Vec<usize>> {
    let mut frames = Vec::with_capacity(count);
    for _ in 0..count {
        match frame::alloc_zeroed() {
            Some(pa) => frames.push(pa),
            None => {
                frames.into_iter().for_each(frame::free);
                return None;
            }
        }
    }
    Some(frames)
}

/// Allocates `size` bytes of the calling program's heap, growing it if
/// need be, and maps any of its pages that aren't yet. Returns its address
/// in the program, or None.
pub fn alloc(size: usize) -> Option<usize> {
    let rounded = UserHeap::rounded(size)?;
    let (ptr, missing) = with_current(|p| {
        let ptr = match p.heap.alloc(size) {
            Some(ptr) => ptr,
            None => {
                // Grow the heap's range; the free range at its end, if
                // any, counts towards it.
                let short = rounded.saturating_sub(p.heap.free_at_end(p.heap_end));
                let grow = page_up(short).max(HEAP_GROW);
                if p.heap_end.checked_add(grow).is_none_or(|end| end > HEAP_LIMIT) {
                    return None;
                }
                p.heap.add(p.heap_end, grow);
                p.heap_end += grow;
                p.heap.alloc(size)?
            }
        };
        Some((ptr, p.memory.missing_pages(ptr, rounded)))
    }).flatten()?;
    let Some(frames) = zeroed_frames(missing) else {
        free(ptr);
        return None;
    };
    // Only the program itself touches its heap, so nothing moved meanwhile.
    with_current(|p| p.memory.map_frames(ptr, rounded, frames, MapFlags::USER_RW));
    Some(ptr)
}

/// Frees one of the calling program's allocations, unmapping any pages
/// that are now entirely free. False if `ptr` isn't an allocation.
pub fn free(ptr: usize) -> bool {
    with_current(|p| {
        let Some((start, len)) = p.heap.release(ptr) else { return false };
        let (first, end) = (page_up(start), page_down(start + len));
        if end > first {
            p.memory.unmap_range(first, end, p.asid);
        }
        true
    }).unwrap_or(false)
}

/// The calling program touched `va`, which isn't mapped: if that's in the
/// stack's range, maps the stack down to there and returns true, so the
/// program can carry on.
pub fn grow_stack(va: usize) -> bool {
    if !(STACK_LIMIT..STACK_TOP).contains(&va) {
        return false;
    }
    let bottom = page_down(va);
    let Some(missing) = with_current(|p| p.memory.missing_pages(bottom, STACK_TOP - bottom)) else { return false };
    if missing == 0 {
        return false;
    }
    let Some(frames) = zeroed_frames(missing) else { return false };
    with_current(|p| p.memory.map_frames(bottom, STACK_TOP - bottom, frames, MapFlags::USER_RW));
    log::trace!("stack grew to {} KiB", (STACK_TOP - bottom) / 1024);
    true
}

/// The calling program's (free, total) heap bytes, counting the memory its
/// heap could still grow into.
pub fn heap_stats() -> Option<(usize, usize)> {
    let growable = frame::stats().0 * PAGE_SIZE;
    with_current(|p| (p.heap.total - p.heap.in_use + growable, p.heap.total + growable))
}

/// How the calling program is connected to the console; None if the
/// caller isn't a program.
pub fn console() -> Option<Console> {
    with_current(|p| p.console)
}

/// The calling program's command-line arguments.
pub fn args() -> Option<String> {
    with_current(|p| p.args.clone())
}

/// How big the calling program's allocation at `ptr` is (rounded up to 16
/// bytes), or None if that isn't one.
pub fn alloc_size(ptr: usize) -> Option<usize> {
    with_current(|p| p.heap.used.get(&ptr).copied()).flatten()
}

/// Whether task `id` is a program that hasn't ended.
pub fn is_running(id: TaskId) -> bool {
    TABLE.lock().processes.iter().any(|p| p.task == id && !p.exited)
}

/// Program `id`'s exit status, once it has ended and been reaped: None
/// while it's still around, Some(None) if there's no such program (or it
/// ended too long ago to remember).
pub fn exit_status(id: TaskId) -> Option<Option<i32>> {
    let table = TABLE.lock();
    if table.processes.iter().any(|p| p.task == id) {
        return None;
    }
    Some(table.statuses.iter().rev().find(|(t, _)| *t == id).map(|&(_, s)| s))
}

/// Records the calling program's exit status (kept as 0-255), unless it
/// already has one.
pub fn set_exit_status(status: i32) {
    with_current(|p| { p.status.get_or_insert(status & 0xff); });
}

fn set_status_of(id: TaskId, status: i32) {
    if let Some(p) = TABLE.lock().processes.iter_mut().find(|p| p.task == id && !p.exited) {
        p.status.get_or_insert(status);
    }
}

// ---- file descriptors --------------------------------------------------------
// These hand back any descriptor they replace or remove, for the caller to
// drop once the table's lock is released: closing a file can wait for the
// disk.

/// The calling program's descriptor `fd`.
pub fn fd_get(fd: u64) -> Option<Fd> {
    with_current(|p| p.fds.get(fd as usize).cloned().flatten()).flatten()
}

/// Gives `object` the calling program's lowest free descriptor number.
pub fn fd_add(object: Fd) -> Result<u64, Fd> {
    let mut object = Some(object);
    let added = with_current(|p| {
        let n = match p.fds.iter().position(Option::is_none) {
            Some(n) => n,
            None if p.fds.len() < MAX_FDS => { p.fds.push(None); p.fds.len() - 1 }
            None => return None,
        };
        p.fds[n] = object.take();
        Some(n as u64)
    }).flatten();
    added.ok_or_else(|| object.take().unwrap())
}

/// Closes descriptor `fd`; None if it wasn't open.
pub fn fd_take(fd: u64) -> Option<Fd> {
    with_current(|p| p.fds.get_mut(fd as usize).and_then(Option::take)).flatten()
}

/// Makes descriptor `to` refer to `object`, giving back what it was.
/// Err if `to` is out of range.
pub fn fd_set(to: u64, object: Fd) -> Result<Option<Fd>, Fd> {
    let to = to as usize;
    if to >= MAX_FDS {
        return Err(object);
    }
    let mut object = Some(object);
    let old = with_current(|p| {
        if p.fds.len() <= to {
            p.fds.resize(to + 1, None);
        }
        core::mem::replace(&mut p.fds[to], object.take())
    });
    old.ok_or_else(|| object.take().unwrap())
}

/// Closes every descriptor of the calling program (it's exiting).
pub fn fd_take_all() -> Vec<Fd> {
    with_current(|p| core::mem::take(&mut p.fds).into_iter().flatten().collect()).unwrap_or_default()
}

/// Keeps `reply` for the calling program (replacing the last one).
pub fn set_http(reply: Option<HttpReply>) {
    with_current(|p| p.http = reply);
}

/// Runs `f` on the calling program's last HTTP response.
pub fn with_http<R>(f: impl FnOnce(Option<&HttpReply>) -> R) -> Option<R> {
    with_current(|p| f(p.http.as_ref()))
}

/// Records why the calling program's last call failed.
pub fn set_error(why: &str) {
    with_current(|p| p.last_error = String::from(why));
}

pub fn last_error() -> Option<String> {
    with_current(|p| p.last_error.clone())
}

/// Records that the calling program opened window `id`, so it's closed
/// when the program ends.
pub fn add_window(id: WindowId, kind: WindowKind) {
    with_current(|p| p.windows.push((id, kind)));
}

/// Whether the caller may use window `id`: kernel code may use any, a
/// program only its own.
pub fn owns_window(id: WindowId, kind: WindowKind) -> bool {
    with_current(|p| p.windows.contains(&(id, kind))).unwrap_or(true)
}

/// Forgets a window the calling program closed itself.
pub fn remove_window(id: WindowId) {
    with_current(|p| p.windows.retain(|&(w, _)| w != id));
}

/// Ctrl+C: kills the programs started with `run` (several, for a
/// pipeline), if any are going. Returns whether there were any. Safe from
/// IRQ context (it doesn't allocate).
pub fn interrupt_foreground() -> bool {
    let mut ids = [0 as TaskId; 16];
    let mut count = 0;
    for p in TABLE.lock().processes.iter_mut().filter(|p| p.console == Console::Foreground && !p.exited) {
        if count < ids.len() {
            p.status.get_or_insert(STATUS_INTERRUPTED);
            ids[count] = p.task;
            count += 1;
        }
    }
    if count > 0 {
        INTERRUPTED.store(true, Ordering::Release);
    }
    for &id in &ids[..count] {
        scheduler::kill(id);
    }
    count > 0
}

/// Whether the last foreground program was ended by Ctrl+C (and forgets it).
pub fn take_interrupted() -> bool {
    INTERRUPTED.swap(false, Ordering::AcqRel)
}

/// Whether a program started with `run` is still going, or hasn't been
/// reaped yet (so its files may not be written back).
pub fn foreground_running() -> bool {
    TABLE.lock().processes.iter().any(|p| p.console == Console::Foreground)
}

/// A running program, for `ps` and the Task Manager.
pub struct ProcessInfo {
    pub pid: Pid,
    pub task: TaskId,
    pub console: Console,
    pub heap_used: usize,
    /// All the memory mapped for it: image, heap and stack.
    pub memory: usize,
    pub windows: usize,
}

pub fn list() -> Vec<ProcessInfo> {
    TABLE.lock().processes.iter()
        .filter(|p| !p.exited)
        .map(|p| ProcessInfo {
            pid: p.pid,
            task: p.task,
            console: p.console,
            heap_used: p.heap.in_use,
            memory: p.memory.pages.len() * PAGE_SIZE,
            windows: p.windows.len(),
        })
        .collect()
}

/// Ends program `id`. Kernel tasks can't be killed.
pub fn kill(id: TaskId) -> Result<(), &'static str> {
    let is_program = TABLE.lock().processes.iter().any(|p| p.task == id && !p.exited);
    if !is_program {
        return Err(if scheduler::tasks().iter().any(|t| t.id == id) { "that's a kernel task, not a program" } else { "no such task" });
    }
    set_status_of(id, STATUS_KILLED);
    scheduler::kill(id);
    Ok(())
}

/// Whether all of `ptr..ptr+len` is mapped in the calling program's
/// address space (growing the stack first if it's on the stack). During
/// its syscalls TTBR0 holds that address space, so the kernel can then
/// read and write such a range directly. (Every page a program has is
/// writable.)
pub fn user_range_ok(ptr: usize, len: usize) -> bool {
    let len = len.max(1);
    let Some(end) = ptr.checked_add(len) else { return false };
    let mapped = || with_current(|p| p.memory.is_mapped(ptr, len)).unwrap_or(false);
    // A buffer on the stack, below where the program has touched yet.
    mapped() || (ptr >= STACK_LIMIT && end <= STACK_TOP && grow_stack(ptr) && mapped())
}

#[cfg(feature = "test")]
pub mod tests {
    use super::*;

    const START: usize = USER_BASE + 0x10000;

    fn test_heap_alloc_is_aligned_and_in_range() {
        let mut heap = UserHeap::new(START, START + 0x10000);
        for size in [1, 15, 16, 17, 100, 4096] {
            let p = heap.alloc(size).unwrap();
            assert!(p % 16 == 0 && p >= START && p + size <= START + 0x10000);
        }
    }

    fn test_heap_allocations_dont_overlap() {
        let mut heap = UserHeap::new(START, START + 0x10000);
        let a = heap.alloc(100).unwrap();
        let b = heap.alloc(100).unwrap();
        assert!(b >= a + 100 || a >= b + 100);
    }

    fn test_heap_runs_out_and_recovers() {
        let mut heap = UserHeap::new(START, START + 1024);
        let a = heap.alloc(1024).unwrap();
        assert!(heap.alloc(16).is_none());
        assert!(heap.free(a));
        assert_eq!(heap.alloc(1024), Some(a));
    }

    fn test_heap_free_merges_neighbours() {
        let mut heap = UserHeap::new(START, START + 3 * 256);
        let a = heap.alloc(256).unwrap();
        let b = heap.alloc(256).unwrap();
        let c = heap.alloc(256).unwrap();
        // Freed out of order, the three must merge back into one range.
        assert!(heap.free(a) && heap.free(c) && heap.free(b));
        assert_eq!(heap.free.len(), 1);
        assert_eq!(heap.in_use, 0);
        assert_eq!(heap.alloc(3 * 256), Some(START));
    }

    fn test_heap_rejects_bad_frees() {
        let mut heap = UserHeap::new(START, START + 1024);
        let a = heap.alloc(64).unwrap();
        assert!(!heap.free(a + 16), "freeing inside an allocation");
        assert!(!heap.free(0x1234));
        assert!(heap.free(a));
        assert!(!heap.free(a), "double free");
    }

    fn test_heap_huge_request_fails() {
        let mut heap = UserHeap::new(START, START + 1024);
        assert!(heap.alloc(usize::MAX).is_none());
        assert!(heap.alloc(usize::MAX - 8).is_none());
    }

    fn test_heap_grows_into_added_memory() {
        let mut heap = UserHeap::new(START, START + 256);
        let a = heap.alloc(128).unwrap();
        assert!(heap.alloc(256).is_none());
        assert_eq!(heap.free_at_end(START + 256), 128);
        // Growing merges the new memory with the free range at the end.
        heap.add(START + 256, 256);
        assert_eq!(heap.free_at_end(START + 512), 384);
        assert_eq!(heap.alloc(384), Some(a + 128));
    }

    fn test_memory_maps_zeroed_pages() {
        let mut mem = Memory::new();
        assert!(mem.map_range(START + 100, 2 * PAGE_SIZE, MapFlags::USER_RW));
        // 100 bytes in, two pages long: that touches three pages.
        assert_eq!(mem.pages.len(), 3);
        assert!(mem.is_mapped(START, 3 * PAGE_SIZE));
        assert!(!mem.is_mapped(START, 3 * PAGE_SIZE + 1));
        assert!(!mem.is_mapped(START - 1, 2));
        let pa = mem.page_at(START + PAGE_SIZE).unwrap();
        assert!(unsafe { core::slice::from_raw_parts(phys_to_virt(pa) as *const u8, PAGE_SIZE) }.iter().all(|&b| b == 0));
        // Mapping again maps nothing new.
        assert!(mem.map_range(START, PAGE_SIZE, MapFlags::USER_RW));
        assert_eq!(mem.pages.len(), 3);
    }

    fn test_memory_writes_across_pages() {
        let mut mem = Memory::new();
        assert!(mem.map_range(START, 2 * PAGE_SIZE, MapFlags::USER_RW));
        let bytes: Vec<u8> = (0..64).collect();
        assert!(mem.write(START + PAGE_SIZE - 32, &bytes));
        let first = mem.page_at(START).unwrap();
        let second = mem.page_at(START + PAGE_SIZE).unwrap();
        let tail = unsafe { core::slice::from_raw_parts((phys_to_virt(first) + PAGE_SIZE - 32) as *const u8, 32) };
        let head = unsafe { core::slice::from_raw_parts(phys_to_virt(second) as *const u8, 32) };
        assert_eq!(tail, &bytes[..32]);
        assert_eq!(head, &bytes[32..]);
        assert!(!mem.write(START + 2 * PAGE_SIZE - 8, &bytes), "running off the mapped pages");
    }

    fn test_memory_gives_its_frames_back() {
        let (free_before, _) = frame::stats();
        {
            let mut mem = Memory::new();
            assert!(mem.map_range(START, 64 * PAGE_SIZE, MapFlags::USER_RW));
            assert!(frame::stats().0 < free_before - 64);
        }
        assert_eq!(frame::stats().0, free_before, "pages or page tables leaked");
    }

    fn test_new_process_has_stack_and_exit_stub() {
        let (free_before, _) = frame::stats();
        {
            let new = NewProcess::new("test", Console::Detached).unwrap();
            assert!(new.memory.is_mapped(STACK_TOP - STACK_INITIAL, STACK_INITIAL));
            assert!(!new.memory.is_mapped(STACK_TOP - STACK_INITIAL - PAGE_SIZE, 1), "more stack than it starts with");
            let stub = new.memory.page_at(EXIT_STUB).unwrap();
            assert_eq!(unsafe { *(phys_to_virt(stub) as *const [u32; 2]) }, EXIT_CODE);
            assert!(!new.memory.is_mapped(USER_BASE, 1), "nothing loaded yet");
        }
        assert_eq!(frame::stats().0, free_before);
    }

    pub fn run() {
        crate::test::run_all(&[
            &test_heap_alloc_is_aligned_and_in_range,
            &test_heap_allocations_dont_overlap,
            &test_heap_runs_out_and_recovers,
            &test_heap_free_merges_neighbours,
            &test_heap_rejects_bad_frees,
            &test_heap_huge_request_fails,
            &test_heap_grows_into_added_memory,
            &test_memory_maps_zeroed_pages,
            &test_memory_writes_across_pages,
            &test_memory_gives_its_frames_back,
            &test_new_process_has_stack_and_exit_stub,
        ]);
    }
}
