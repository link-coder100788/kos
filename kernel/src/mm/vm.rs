//! The kernel's address spaces.
//!
//! - Kernel (TTBR1): all of physical memory and MMIO at `KERNEL_OFFSET +
//!   physical`, with per-region permissions.
//! - Low half (TTBR0) for kernel tasks: just the boot stubs, which
//!   secondary cores run at their physical addresses. Programs each get
//!   their own low half instead (`process`).

use core::ptr::addr_of;
use crate::arch::mmu::{self, AddressSpace, MapFlags};
use crate::mm::layout::*;
use crate::println;
use crate::sections::{__boot_end, __boot_start, __data_start, __rodata_end, __rodata_start, __text_end, __text_start};
use crate::std::lock::mutex::Mutex;

pub static KERNEL_SPACE: Mutex<Option<AddressSpace>> = Mutex::new(None);
pub static USER_SPACE: Mutex<Option<AddressSpace>> = Mutex::new(None);

/// Maps physical `start..end` at its kernel virtual address.
fn linear(space: &mut AddressSpace, start: usize, end: usize, flags: MapFlags, what: &str) {
    log::debug!("  {:#018x}..{:#018x} {} {}", phys_to_virt(start), phys_to_virt(end), flags, what);
    space.map(phys_to_virt(start), start, end - start, flags);
}

/// Maps physical `start..end` at the same virtual address.
fn identity(space: &mut AddressSpace, start: usize, end: usize, flags: MapFlags, what: &str) {
    log::debug!("  {:#018x}..{:#018x} {} {}", start, end, flags, what);
    space.map(start, start, end - start, flags);
}

/// Builds the kernel and user address spaces and switches the boot core to
/// them. Secondary cores pick them up with `arch::mmu::enable_on_this_core`.
pub fn init() {
    let phys = |sym: *const u8| virt_to_phys(sym as usize);
    let text_start = phys(addr_of!(__text_start));
    let text_end = phys(addr_of!(__text_end));
    let rodata_start = phys(addr_of!(__rodata_start));
    let rodata_end = phys(addr_of!(__rodata_end));
    let data_start = phys(addr_of!(__data_start));
    let boot_start = phys(addr_of!(__boot_start));
    let boot_end = phys(addr_of!(__boot_end));

    let mut kernel = AddressSpace::new();
    log::debug!("kernel address space (TTBR1):");
    linear(&mut kernel, MMIO_START, MMIO_END, MapFlags::DEVICE, "mmio");
    linear(&mut kernel, RAM_START, text_start, MapFlags::KERNEL_RW, "dtb + boot stacks");
    linear(&mut kernel, text_start, text_end, MapFlags::KERNEL_RX, "kernel text");
    linear(&mut kernel, rodata_start, rodata_end, MapFlags::KERNEL_RO, "kernel rodata");
    linear(&mut kernel, data_start, RAM_END, MapFlags::KERNEL_RW, "kernel data, heap and page frames");

    let mut user = AddressSpace::new();
    log::debug!("user address space (TTBR0):");
    identity(&mut user, boot_start, boot_end, MapFlags::KERNEL_RX, "boot stubs");
    // Programs aren't mapped here: each gets its own TTBR0 table (see
    // `process`), and kernel tasks only see the boot stubs.

    mmu::enable(&kernel, &user);
    *KERNEL_SPACE.lock() = Some(kernel);
    *USER_SPACE.lock() = Some(user);
    log::info!("MMU on: kernel in the higher half, {} page frames used for tables", crate::mm::frame::stats().1 - crate::mm::frame::stats().0);
}

/// Software walk of whichever half `va` belongs to.
pub fn translate(va: usize) -> Option<usize> {
    let space = if va >= KERNEL_OFFSET { &KERNEL_SPACE } else { &USER_SPACE };
    space.lock().as_ref().and_then(|s| s.translate(va))
}
