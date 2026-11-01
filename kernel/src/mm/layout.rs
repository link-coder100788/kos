//! Memory layout.
//!
//! Physical memory (QEMU `virt` with `-m 512M`):
//!
//! ```text
//! 0x0000_0000 .. 0x0800_0000   flash (the bootloader runs from here)
//! 0x0800_0000 .. 0x4000_0000   MMIO: GIC, UART, fw_cfg, virtio, ...
//! 0x4000_0000 .. 0x4020_0000   DTB + boot stacks for every core
//! 0x4020_0000 .. _end          kernel image
//! _end        .. RAM_END       shared by the kernel heap and page frames:
//!                              the heap starts at _end (INITIAL_HEAP) and
//!                              grows up; mm::frame hands out frames for
//!                              page tables and programs from the top down
//! ```
//!
//! So neither has a fixed share: the heap can use all the memory programs
//! aren't, and the other way round. (The heap never shrinks, though.)
//!
//! Virtual memory:
//!
//! ```text
//! 0x0000_0000_0000_0000..      TTBR0: the running program's own address
//!                              space (see `process`), or just the boot
//!                              stubs for kernel tasks
//! 0xFFFF_FF80_0000_0000..      TTBR1: everything physical, at
//!                              KERNEL_OFFSET + physical address. The
//!                              kernel is linked to run here.
//! ```

pub const PAGE_SIZE: usize = 4096;

/// Where physical address 0 appears in the kernel's (TTBR1) half. Must
/// match `KERNEL_OFFSET` in linker.ld and arch/boot.s.
pub const KERNEL_OFFSET: usize = 0xFFFF_FF80_0000_0000;

pub const FLASH_START: usize = 0x0000_0000;
pub const FLASH_END: usize = 0x0800_0000;
pub const MMIO_START: usize = 0x0800_0000;
pub const MMIO_END: usize = 0x4000_0000;

pub const RAM_START: usize = 0x4000_0000;
pub const RAM_END: usize = 0x6000_0000;

/// The heap's size at boot; it grows from there (`mm::allocator`).
pub const INITIAL_HEAP: usize = 32 * 1024 * 1024;
/// How much the heap grows by at a time, at least.
pub const HEAP_GROW: usize = 8 * 1024 * 1024;

/// The kernel virtual address of physical address `pa`.
#[inline]
pub const fn phys_to_virt(pa: usize) -> usize {
    pa + KERNEL_OFFSET
}

/// The physical address behind kernel or user virtual address `va`, for
/// handing buffers to devices. Kernel addresses are a fixed offset from
/// physical; low addresses (the boot stubs) are identity mapped. Program
/// addresses aren't handled: copy program data into the kernel first.
#[inline]
pub const fn virt_to_phys(va: usize) -> usize {
    if va >= KERNEL_OFFSET { va - KERNEL_OFFSET } else { va }
}
