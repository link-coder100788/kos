#![no_std]
#![no_main]
#![allow(function_casts_as_integer)]
#![allow(static_mut_refs)]
#![allow(unused)]
#![allow(unsafe_op_in_unsafe_fn)]
#![feature(allocator_api)]
#![allow(improper_ctypes)]
#![feature(negative_impls)]
#![allow(private_interfaces)]
#![deny(warnings)]

//! KOS kernel entry point.
//!
//! `rust_main` runs the early/middle/late initcalls (see `init::calls`) and
//! then hands core 0 to the scheduler.

mod arch;
mod assistant;
mod crypto;
mod drivers;
mod filesys;
mod init;
mod k;
mod loader;
mod klog;
mod mm;
mod net;
mod module;
mod prelude;
mod process;
mod rand;
mod scheduler;
mod sections;
mod shell;
mod sound;
mod stats;
mod std;
mod syscall;
mod test;
mod ui;
mod util;

extern crate alloc;
extern crate base64;
extern crate elf;
extern crate fatfs;
extern crate lazy_static;
extern crate smoltcp;

use alloc::vec::Vec;
use core::arch::asm;
use core::ptr::addr_of;
use core::ptr::write_volatile;
use arch::{exceptions, gic, timer};
use mm::allocator;
use sections::_end;

#[unsafe(no_mangle)]
#[cfg(not(feature = "test_exec"))]
#[cfg(not(feature = "test_alloc_overflow"))]
pub extern "C" fn rust_main() -> ! {
    unsafe { write_volatile(mm::layout::phys_to_virt(0x0900_0000) as *mut u32, b'@' as u32); }
    id_println!("--- Kernel Start ---");
    unsafe { init::early(); }
    unsafe { init::middle(); }
    unsafe { init::late(); }
    scheduler::run();
    loop {}
}

#[unsafe(no_mangle)]
#[cfg(feature = "test_exec")]
pub extern "C" fn rust_main() -> ! {
    unsafe { write_volatile(mm::layout::phys_to_virt(0x0900_0000) as *mut u32, b'!' as u32); }
    init::calls::r_zero_bss();
    println!("--- Kernel Start --- !TESTING__EXEC!");
    let el: u64;
    unsafe { asm!("mrs {0}, CurrentEL", out(reg) el); }
    println!("CurrentEL = {}", (el >> 2) & 0b11);
    exceptions::init();
    println!("Vector table set up");
    gic::init();
    timer::init();
    println!("Timer initialized");
    unsafe { asm!("udf #0"); } // FAILPOINT
    loop {}
}

#[unsafe(no_mangle)]
#[cfg(feature = "test_alloc_overflow")]
pub extern "C" fn rust_main() -> ! {
    unsafe { write_volatile(mm::layout::phys_to_virt(0x0900_0000) as *mut u32, b'!' as u32); }
    init::calls::r_zero_bss();
    println!("--- Kernel Start --- !TESTING__ALLOC_OVERFLOW!");
    let el: u64;
    unsafe { asm!("mrs {0}, CurrentEL", out(reg) el); }
    println!("CurrentEL = {}", (el >> 2) & 0b11);
    exceptions::init();
    println!("Vector table set up");
    gic::init();
    timer::init();
    println!("Timer initialized");
    let heap_start = (addr_of!(_end) as usize + 15) & !15;
    let heap_size = mm::layout::INITIAL_HEAP;
    assert!(heap_size > 0, "heap_size must be greater than 0");
    println!("Heap Start: {:#x}, Size: {:#x} ({} bytes / {:.2} kb / ~{:.5} mb)", heap_start, heap_size, heap_size, heap_size as f64 / 1024.0, heap_size as f64 / 1024.0 / 1024.0);
    unsafe { allocator::init(heap_start, heap_size); }
    let mut test_vec: Vec<u8> = Vec::new();
    loop {
        for i in 0..=u8::MAX {
            test_vec.push(i);
        }
    }
    loop {}
}

#[unsafe(no_mangle)]
pub extern "C" fn test_task_a() -> ! {
    loop {
        println!("A");
        for _ in 0..3_000_000 { unsafe { core::arch::asm!("nop"); } }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn test_task_b() -> ! {
    loop {
        println!("B");
        for _ in 0..3_000_000 { unsafe { core::arch::asm!("nop"); } }
    }
}

