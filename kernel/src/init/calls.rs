//! Boot-time initcalls. Each `#[initcall]` fn is placed in a linker section
//! by `kos_derive` and run in (level, priority) order by `init::early`,
//! `init::middle` and `init::late`.

use alloc::boxed::Box;
use core::arch::asm;
use core::ptr::addr_of;
use kos_derive::initcall;
use crate::arch::{exceptions, gic, psci, smp, stacktrace, timer};
use crate::arch::timer::delay_ms;
use crate::mm::{self, allocator};
use crate::sections::*;
use crate::{filesys, k, loader, println, scheduler, shell, ui};

#[initcall(level = InitLevel::early, priority = 1)]
pub fn r_zero_bss() {
    unsafe {
        let mut current = core::ptr::addr_of_mut!(__bss_start);
        let end = core::ptr::addr_of!(__bss_end) as *mut u8;
        while current < end {
            core::ptr::write_volatile(current, 0);
            current = current.add(1);
        }
    }
}

#[initcall(level = InitLevel::early, priority = 2)]
fn kernel_log() {
    crate::klog::init();
    log::info!("KOS starting");
}

#[initcall(level = InitLevel::early, priority = 3)]
fn vector_table() {
    exceptions::init();
    log::debug!("exception vectors installed");
}

#[initcall(level = InitLevel::early, priority = 4)]
fn gic_timer() {
    gic::init();
    timer::init();
    crate::drivers::uart::init_irq();
}

#[initcall(level = InitLevel::early, priority = 5)]
fn cpu_info() {
    let (el, id): (u64, u64);
    unsafe { asm!("mrs {0}, CurrentEL", "mrs {1}, mpidr_el1", out(reg) el, out(reg) id) }
    log::debug!("running at EL{} on core {}", (el >> 2) & 3, id & 0xff);
    log::debug!(
        "kernel sections: initcall/syscall tables {:#x}..{:#x}, secondary entry {:#x}, tertiary entry {:#x}",
        addr_of!(___fn_section_start) as usize, addr_of!(___fn_section_end) as usize,
        addr_of!(__secondary_entry_start) as usize, addr_of!(__tertiary_entry_start) as usize,
    );
    if log::log_enabled!(log::Level::Trace) {
        println!("{}", stacktrace::RegisterDump::capture());
    }
}

#[initcall(level = InitLevel::early, priority = 10)]
fn mmu() {
    // The heap's first stretch follows the kernel; page frames are
    // everything after it (the heap can claim more later).
    let heap_start = (addr_of!(_end) as usize + 15) & !15;
    mm::frame::init(mm::layout::virt_to_phys(heap_start + mm::layout::INITIAL_HEAP));
    mm::vm::init();
}

#[initcall(level = InitLevel::middle, priority = 1)]
fn start_allocator() {
    let heap_start = (addr_of!(_end) as usize + 15) & !15;
    // It ends on a page boundary, where the frames `mmu` left free begin.
    let heap_end = (heap_start + mm::layout::INITIAL_HEAP + mm::layout::PAGE_SIZE - 1) & !(mm::layout::PAGE_SIZE - 1);
    let heap_size = heap_end - heap_start;
    unsafe { allocator::init(heap_start, heap_size); }
    log::info!("heap: {} MiB at {:#x}..{:#x}, growing into free memory", heap_size / (1024 * 1024), heap_start, heap_start + heap_size);
}

#[initcall(level = InitLevel::middle, priority = 2)]
fn test_box() {
    let boxed = Box::new(42);
    assert_eq!(*boxed, 42);
    log::debug!("heap allocation works");
}

#[initcall(level = InitLevel::middle, priority = 3)]
fn wake_secondary_cores() {
    log::debug!("waking secondary cores");
    // The bootloader knows how to route cores 1..=4 (see smp.rs).
    for core_id in 1..=4 {
        let bootloader_entry = 0x0;
        let ret = unsafe { psci::psci_cpu_on(core_id, bootloader_entry, 0) };
        if ret != 0 {
            // INVALID_PARAMETERS (-2) means there's no such core: QEMU was
            // started with fewer CPUs.
            log::info!("no core {} (PSCI error {}): {} secondary cores", core_id, ret, core_id - 1);
            break;
        }
        let start = timer::Instant::now();
        while !smp::is_online(core_id) {
            if start.elapsed().as_millis() > 1000 {
                log::error!("core {} woke but never reached its entry loop", core_id);
                break;
            }
            core::hint::spin_loop();
        }
    }
    // Core 1 runs std::thread tasks, core 2 async tasks and cores 3-4
    // programs (see arch::smp). Core 0 does any of it that's missing.
    log::info!("{} of 5 cores online", (0..5).filter(|&c| smp::is_online(c)).count());
    if crate::rand::available() {
        log::info!("rand: ChaCha20 seeded by the RNDR hardware RNG, sample {:#018x}", crate::rand::u64());
    } else {
        log::warn!("rand: no RNDR on this CPU; TLS won't work");
    }
    if !smp::is_online(1) {
        log::info!("no thread core; running thread tasks on core 0");
        scheduler::spawn("thread worker", smp::thread_worker_task);
    }
    if !smp::is_online(2) {
        log::info!("no async core; running async tasks on core 0");
        scheduler::spawn("async worker", smp::async_worker_task);
    }
    if !scheduler::PROGRAM_CORES.iter().any(|&c| smp::is_online(c as u64)) {
        log::info!("no program cores; running programs on core 0");
    }
}

#[initcall(level = InitLevel::middle, priority = 5)]
fn input_devices() {
    crate::drivers::virtio::input::init();
}

#[initcall(level = InitLevel::middle, priority = 5)]
fn sound() {
    crate::sound::init();
}

#[initcall(level = InitLevel::middle, priority = 4)]
fn test_if_enabled() {
    if cfg!(feature = "test") {
        println!("Running tests");
        #[cfg(feature = "test")]
        k::aarch64_codegen::tests::run();
        #[cfg(feature = "test")]
        k::ast::tests::run();
        #[cfg(feature = "test")]
        k::encoder::tests::run();
        #[cfg(feature = "test")]
        k::ir::tests::run();
        #[cfg(feature = "test")]
        k::kbin::tests::run();
        #[cfg(feature = "test")]
        k::lexer::tests::run();
        #[cfg(feature = "test")]
        k::parser::tests::run();
        #[cfg(feature = "test")]
        k::typecheck::tests::run();
        #[cfg(feature = "test")]
        k::test::full_test::run();
        #[cfg(feature = "test")]
        crate::process::tests::run();
        #[cfg(feature = "test")]
        crate::rand::tests::run();
        #[cfg(feature = "test")]
        crate::crypto::tests::run();
        #[cfg(feature = "test")]
        crate::test::run_all(&[&crate::test::heap_stress_test, &crate::test::heap_grows_and_shrinks_test]);
        // The rest need the scheduler; this task runs them, then powers off.
        #[cfg(feature = "test")]
        scheduler::spawn("tests", crate::test::test_task);
    } else {
        log::debug!("in-kernel tests not enabled (build with FEATURES=test)");
    }
}

#[initcall(level = InitLevel::late, priority = 1)]
fn scheduler() {
    scheduler::spawn("shell", shell::uart_read_task);
    scheduler::spawn("fs", filesys::fs_task::fs_task);
    scheduler::spawn("loader", loader::user_task_runner_1);
    scheduler::spawn("net", crate::net::net_task);
    // Elliptic-curve maths and TLS records are stack-hungry.
    scheduler::spawn_with_stack("tls", crate::net::tls_task, 256 * 1024);
    scheduler::spawn("ui", ui::task::ui_task);
}
