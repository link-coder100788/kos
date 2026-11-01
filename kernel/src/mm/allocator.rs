//! The kernel heap: a first-fit free-list allocator, starting at `_end`.
//!
//! It starts at `INITIAL_HEAP` bytes and grows when nothing fits: it
//! claims the page frames just above its end (`frame::claim`), which stay
//! free as long as possible because frames are handed out from the top of
//! RAM down. So the heap and the page frames share memory with no fixed
//! split. When more than `2 * HEAP_GROW` at its end is free, it shrinks,
//! giving all but `HEAP_GROW` of that back (`frame::release`), though
//! never below where it started. The gap between the two keeps it from
//! growing and shrinking over and over.
//!
//! Free blocks form a singly linked list sorted by address, with each
//! block's header stored in the free memory itself. Freed blocks are merged
//! with free neighbours so the heap doesn't fragment into slivers. Every
//! block boundary is 16-byte aligned and every size a multiple of 16, so
//! splitting never leaves a remainder too small to hold a header.
//!
//! The lock is taken with IRQs masked: otherwise a task preempted while
//! holding it would stall every other allocation on its core (and the
//! scheduler frees exited tasks' stacks from the timer IRQ).

use alloc::vec::Vec;
use core::alloc::GlobalAlloc;
use core::alloc::Layout;
use core::alloc::Allocator;
use core::alloc::AllocError;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};
use core::ptr::NonNull;
use crate::arch::irq;
use crate::mm::frame;
use crate::mm::layout::{virt_to_phys, HEAP_GROW, PAGE_SIZE};
use crate::println;

#[derive(Copy, Clone, Debug)]
pub struct Allocation {
    pub start: usize,
    pub size: usize,
    pub layout: Layout,
}

#[derive(Default)]
pub struct AllocationLog {
   pub allocations: [Option<Allocation>; 5],
}

impl AllocationLog {
    fn push(&mut self, allocation: Allocation) {
        for i in 0..4 {
            self.allocations[i] = self.allocations[i + 1].take();
        }
        self.allocations[4] = Some(allocation);
    }
}

const ALIGN: usize = 16;

struct FreeBlock {
    size: usize,
    next: *mut FreeBlock,
}

struct Heap {
    head: *mut FreeBlock,
    /// Where the heap ends now; it grows from here.
    end: usize,
    /// Where it ended at boot; it never shrinks below this.
    initial_end: usize,
    total: usize,
    used: usize,
    recent: AllocationLog,
}

pub struct KernelHeap {
    locked: AtomicBool,
    heap: UnsafeCell<Heap>,
}

unsafe impl Sync for KernelHeap {}

#[global_allocator]
pub static ALLOCATOR: KernelHeap = KernelHeap {
    locked: AtomicBool::new(false),
    heap: UnsafeCell::new(Heap {
        head: core::ptr::null_mut(),
        end: 0,
        initial_end: 0,
        total: 0,
        used: 0,
        recent: AllocationLog { allocations: [None; 5] },
    }),
};

const fn align_up(x: usize, align: usize) -> usize {
    (x + align - 1) & !(align - 1)
}

/// The block size actually used for `layout`.
fn block_size(layout: &Layout) -> usize {
    align_up(layout.size().max(size_of::<FreeBlock>()), ALIGN)
}

pub unsafe fn init(start: usize, size: usize) {
    let start = align_up(start, ALIGN);
    let size = size & !(ALIGN - 1);
    ALLOCATOR.with(|heap| {
        let block = start as *mut FreeBlock;
        block.write(FreeBlock { size, next: core::ptr::null_mut() });
        heap.head = block;
        heap.end = start + size;
        heap.initial_end = start + size;
        heap.total = size;
        heap.used = 0;
    });
}

impl KernelHeap {
    fn with<R>(&self, f: impl FnOnce(&mut Heap) -> R) -> R {
        irq::without_irqs(|| {
            while self.locked.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
                core::hint::spin_loop();
            }
            let result = f(unsafe { &mut *self.heap.get() });
            self.locked.store(false, Ordering::Release);
            result
        })
    }

    /// (percent free, bytes free, total bytes), counting the memory the
    /// heap could still grow into as free.
    pub fn heap_remaining(&self) -> (f64, u64, u64) {
        let (free, total, end) = self.with(|heap| (heap.total - heap.used, heap.total, heap.end));
        let growable = frame::free_bytes_at(virt_to_phys(end));
        let (free, total) = (free + growable, total + growable);
        let percentage = if total > 0 { free as f64 / total as f64 * 100.0 } else { 0.0 };
        (percentage, free as u64, total as u64)
    }

    /// The heap's current size and how much of it is in use, in bytes.
    pub fn heap_size(&self) -> (usize, usize) {
        self.with(|heap| (heap.total, heap.used))
    }

    pub fn last_allocations(&self) -> Vec<Allocation> {
        let recent = self.with(|heap| heap.recent.allocations);
        recent.iter().flatten().copied().collect()
    }
}

impl Heap {
    unsafe fn alloc(&mut self, layout: Layout) -> *mut u8 {
        let size = block_size(&layout);
        let align = layout.align().max(ALIGN);
        let mut prev: *mut FreeBlock = core::ptr::null_mut();
        let mut cur = self.head;
        while !cur.is_null() {
            let block_start = cur as usize;
            let block_end = block_start + (*cur).size;
            let alloc_start = align_up(block_start, align);
            let alloc_end = alloc_start + size;
            if alloc_end <= block_end {
                let next = (*cur).next;
                // Whatever is left before and after the allocation stays free.
                let mut link = next;
                if block_end > alloc_end {
                    let tail = alloc_end as *mut FreeBlock;
                    tail.write(FreeBlock { size: block_end - alloc_end, next });
                    link = tail;
                }
                if alloc_start > block_start {
                    (*cur).size = alloc_start - block_start;
                    (*cur).next = link;
                    link = cur;
                }
                if prev.is_null() { self.head = link; } else { (*prev).next = link; }
                self.used += size;
                self.recent.push(Allocation { start: alloc_start, size, layout });
                return alloc_start as *mut u8;
            }
            prev = cur;
            cur = (*cur).next;
        }
        core::ptr::null_mut()
    }

    unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        let size = block_size(&layout);
        self.used -= size;
        let block = self.insert_free(ptr as usize, size);
        if block as usize + (*block).size == self.end {
            self.shrink(block);
        }
    }

    /// Gives back the end of the heap if a lot of it is free. `last` is the
    /// free block that ends where the heap does.
    unsafe fn shrink(&mut self, last: *mut FreeBlock) {
        if (*last).size <= 2 * HEAP_GROW {
            return;
        }
        let new_end = align_up(last as usize + HEAP_GROW, PAGE_SIZE).max(self.initial_end);
        if new_end >= self.end {
            return;
        }
        let released = self.end - new_end;
        (*last).size -= released;
        self.total -= released;
        self.end = new_end;
        frame::release(virt_to_phys(new_end), released / PAGE_SIZE);
        log::debug!("heap shrank to {} MiB", self.total / (1024 * 1024));
    }

    /// Grows the heap by at least enough for `layout`, from the page frames
    /// above its end. Returns whether it could.
    unsafe fn grow(&mut self, layout: &Layout) -> bool {
        let need = block_size(layout) + layout.align().max(ALIGN);
        let need = align_up(need, PAGE_SIZE);
        let start = self.end;
        // A big step if there's room, so the heap doesn't grow a page at a time.
        for size in [need.max(HEAP_GROW), need] {
            if frame::claim(virt_to_phys(start), size / PAGE_SIZE) {
                self.end += size;
                self.total += size;
                self.insert_free(start, size);
                return true;
            }
        }
        false
    }

    /// Adds `start..start+size` to the free list, merging it with its
    /// neighbours. Returns the free block it ended up in.
    unsafe fn insert_free(&mut self, start: usize, size: usize) -> *mut FreeBlock {
        // Find the free blocks on either side of the one being freed.
        let mut prev: *mut FreeBlock = core::ptr::null_mut();
        let mut next = self.head;
        while !next.is_null() && (next as usize) < start {
            prev = next;
            next = (*next).next;
        }
        let block = start as *mut FreeBlock;
        block.write(FreeBlock { size, next });
        if !next.is_null() && start + size == next as usize {
            (*block).size += (*next).size;
            (*block).next = (*next).next;
        }
        if prev.is_null() {
            self.head = block;
        } else if prev as usize + (*prev).size == start {
            (*prev).size += (*block).size;
            (*prev).next = (*block).next;
            return prev;
        } else {
            (*prev).next = block;
        }
        block
    }
}

unsafe impl GlobalAlloc for KernelHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let (ptr, remaining, total) = self.with(|heap| {
            let mut ptr = heap.alloc(layout);
            if ptr.is_null() && heap.grow(&layout) {
                log::debug!("heap grew to {} MiB", heap.total / (1024 * 1024));
                ptr = heap.alloc(layout);
            }
            (ptr, heap.total - heap.used, heap.total)
        });
        if ptr.is_null() {
            log::error!("out of heap: {} bytes (align {}) requested, {} of {} free", layout.size(), layout.align(), remaining, total);
        } else {
            log::trace!("alloc {:#x} size {} align {} ({} free)", ptr as usize, layout.size(), layout.align(), remaining);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.with(|heap| heap.dealloc(ptr, layout));
        log::trace!("free {:#x} size {}", ptr as usize, layout.size());
    }
}

pub struct PoolAllocator<T> {
    memory: NonNull<T>,
    free_list: UnsafeCell<Option<NonNull<Node>>>,
    capacity: usize,
}

pub struct Node {
    next: Option<NonNull<Node>>,
}

impl<T> PoolAllocator<T> {
    pub unsafe fn new(memory: NonNull<T>, cap: usize) -> Self {
        let mut free_list = None;
        for i in 0..cap {
            let node = memory.as_ptr().add(i).cast::<Node>();
            unsafe {
                (*node).next = free_list;
            }
            free_list = Some(NonNull::new_unchecked(node));
        }
        Self {
            memory,
            free_list: UnsafeCell::new(free_list),
            capacity: cap,
        }
    }
}

unsafe impl<T> Allocator for PoolAllocator<T> {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, alloc::alloc::AllocError> {
        log::trace!("pool alloc {} bytes, {:?}", layout.size(), layout);
        if layout.size() > core::mem::size_of::<T>() || layout.align() > core::mem::align_of::<T>() {
            return Err(AllocError);
        }
        let free_list = unsafe { &mut *self.free_list.get() };
        if let Some(node) = free_list.take() {
            *free_list = unsafe { node.as_ref().next };
            Ok(NonNull::slice_from_raw_parts(
                node.cast::<u8>(),
                layout.size(),
            ))
        } else {
            Err(AllocError)
        }
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        let node = ptr.cast::<Node>();
        let free_list = &mut *self.free_list.get();
        (*node.as_ptr()).next = *free_list;
        *free_list = Some(node);
    }
}
