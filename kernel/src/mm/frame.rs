//! Physical page-frame allocator: one bit per 4 KiB frame of RAM. Frames
//! are handed out as physical addresses; use `layout::phys_to_virt` to
//! touch their contents.
//!
//! Everything below the end of the kernel heap's first stretch is marked
//! used at boot (`init`). Above it, frames are handed out from the top of
//! RAM down, so the free memory just above the heap stays in one piece for
//! the heap to grow into (`claim`).
//!
//! The lock masks IRQs, so the heap can call in while holding its own lock
//! (which also masks IRQs) without a preempted task ever holding this one.

use crate::mm::layout::{PAGE_SIZE, RAM_END, RAM_START, phys_to_virt};
use crate::std::lock::irq_mutex::IrqMutex;

const FRAME_COUNT: usize = (RAM_END - RAM_START) / PAGE_SIZE;
const WORDS: usize = FRAME_COUNT / 64;

struct FrameAllocator {
    /// Bit set = frame in use.
    bitmap: [u64; WORDS],
    free: usize,
    /// Every word above this one is full; searches start here.
    hint: usize,
}

static FRAMES: IrqMutex<FrameAllocator> = IrqMutex::new(FrameAllocator {
    bitmap: [u64::MAX; WORDS],
    free: 0,
    hint: WORDS - 1,
});

fn frame_of(pa: usize) -> usize {
    (pa - RAM_START) / PAGE_SIZE
}

fn address_of(frame: usize) -> usize {
    RAM_START + frame * PAGE_SIZE
}

impl FrameAllocator {
    fn is_used(&self, frame: usize) -> bool {
        self.bitmap[frame / 64] & (1 << (frame % 64)) != 0
    }

    fn set(&mut self, frame: usize, used: bool) {
        if used {
            self.bitmap[frame / 64] |= 1 << (frame % 64);
        } else {
            self.bitmap[frame / 64] &= !(1 << (frame % 64));
        }
    }

    /// The highest free frame.
    fn alloc(&mut self) -> Option<usize> {
        for word in (0..=self.hint).rev() {
            let bits = self.bitmap[word];
            if bits != u64::MAX {
                let bit = 63 - (!bits).leading_zeros() as usize;
                self.bitmap[word] |= 1 << bit;
                self.free -= 1;
                self.hint = word;
                return Some(address_of(word * 64 + bit));
            }
        }
        None
    }

    /// The highest run of `count` physically contiguous free frames.
    fn alloc_contiguous(&mut self, count: usize) -> Option<usize> {
        let mut run = 0;
        for frame in (0..FRAME_COUNT).rev() {
            if self.is_used(frame) {
                run = 0;
                continue;
            }
            run += 1;
            if run == count {
                for f in frame..frame + count {
                    self.set(f, true);
                }
                self.free -= count;
                return Some(address_of(frame));
            }
        }
        None
    }

    fn free(&mut self, pa: usize) {
        assert!(pa % PAGE_SIZE == 0 && (RAM_START..RAM_END).contains(&pa), "freeing bad frame {:#x}", pa);
        let frame = frame_of(pa);
        assert!(self.is_used(frame), "double free of frame {:#x}", pa);
        self.set(frame, false);
        self.free += 1;
        self.hint = self.hint.max(frame / 64);
    }

    /// How many free frames start at `frame`, in a row.
    fn free_run(&self, frame: usize) -> usize {
        let mut f = frame;
        while f < FRAME_COUNT && !self.is_used(f) {
            // Whole free words at a time.
            if f % 64 == 0 && f + 64 <= FRAME_COUNT && self.bitmap[f / 64] == 0 {
                f += 64;
            } else {
                f += 1;
            }
        }
        f - frame
    }
}

/// Marks RAM below `first_free` (the kernel image and the heap's first
/// stretch) as used and the rest as free. Call once, before any `alloc`.
pub fn init(first_free: usize) {
    let mut frames = FRAMES.lock();
    let first = frame_of((first_free + PAGE_SIZE - 1) & !(PAGE_SIZE - 1));
    for f in first..FRAME_COUNT {
        frames.set(f, false);
    }
    frames.free = FRAME_COUNT - first;
    frames.hint = WORDS - 1;
}

/// Allocates one frame. Contents are undefined.
pub fn alloc() -> Option<usize> {
    let frame = FRAMES.lock().alloc();
    match frame {
        Some(pa) => log::trace!("frame alloc {:#x}", pa),
        None => log::error!("out of page frames"),
    }
    frame
}

/// Allocates one frame filled with zeroes.
pub fn alloc_zeroed() -> Option<usize> {
    let pa = alloc()?;
    unsafe { core::ptr::write_bytes(phys_to_virt(pa) as *mut u8, 0, PAGE_SIZE) };
    Some(pa)
}

/// Allocates `count` physically contiguous frames (e.g. for DMA buffers).
pub fn alloc_contiguous(count: usize) -> Option<usize> {
    let frames = FRAMES.lock().alloc_contiguous(count);
    log::trace!("frame alloc {} contiguous -> {:x?}", count, frames);
    frames
}

pub fn free(pa: usize) {
    FRAMES.lock().free(pa);
    log::trace!("frame free {:#x}", pa);
}

/// For the heap: takes the `count` frames starting at `pa` if they're all
/// free, and returns whether it did.
pub fn claim(pa: usize, count: usize) -> bool {
    if pa % PAGE_SIZE != 0 || pa < RAM_START || pa + count * PAGE_SIZE > RAM_END {
        return false;
    }
    let mut frames = FRAMES.lock();
    let first = frame_of(pa);
    if frames.free_run(first) < count {
        return false;
    }
    for f in first..first + count {
        frames.set(f, true);
    }
    frames.free -= count;
    true
}

/// For the heap: gives back the `count` frames starting at `pa`, which it
/// had claimed.
pub fn release(pa: usize, count: usize) {
    let mut frames = FRAMES.lock();
    for f in frame_of(pa)..frame_of(pa) + count {
        assert!(frames.is_used(f), "releasing free frame {:#x}", address_of(f));
        frames.set(f, false);
    }
    frames.free += count;
    frames.hint = frames.hint.max((frame_of(pa) + count - 1) / 64);
}

/// How many bytes are free in one piece starting at `pa`: how far the heap
/// could grow from there right now.
pub fn free_bytes_at(pa: usize) -> usize {
    if pa % PAGE_SIZE != 0 || !(RAM_START..RAM_END).contains(&pa) {
        return 0;
    }
    FRAMES.lock().free_run(frame_of(pa)) * PAGE_SIZE
}

/// (free frames, total frames)
pub fn stats() -> (usize, usize) {
    (FRAMES.lock().free, FRAME_COUNT)
}
