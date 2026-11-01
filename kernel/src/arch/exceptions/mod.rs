pub mod restart_core;

use core::arch::global_asm;
use core::arch::asm;
use core::panic::PanicInfo;
use crate::arch::irq::{self, SPSR_I};
use crate::println;
use crate::scheduler;
use crate::scheduler::task::TrapFrame;
use crate::syscall;

global_asm!(include_str!("vector.s"));

unsafe extern "C" { fn exception_vector_table(); }

pub fn init() {
    unsafe {
        let addr = exception_vector_table as usize as u64;
        asm!("msr vbar_el1, {0}", "isb", in(reg) addr);
    }
}

fn ec_name(ec: u64) -> &'static str {
    match ec {
        0b000000 => "Unknown",
        0b000111 => "SIMD/FP trap (CPACR_EL1 not enabled)",
        0b100101 => "Data abort (same EL)",
        0b100100 => "Data abort (lower EL)",
        0b100001 => "Instruction abort (same EL)",
        0b100000 => "Instruction abort (lower EL)",
        0b010101 => "SVC (AArch64)",
        0b011000 => "MSR/MRS/system instruction trap",
        _ => "Other",
    }
}

fn read_esr_far() -> (u64, u64) {
    let (esr, far): (u64, u64);
    unsafe { asm!("mrs {0}, esr_el1", "mrs {1}, far_el1", out(reg) esr, out(reg) far) };
    (esr, far)
}

/// Syscalls and faults. Returns the frame to resume.
#[unsafe(no_mangle)]
pub extern "C" fn rust_sync_handler(frame: &mut TrapFrame) -> *mut TrapFrame {
    let (esr, far) = read_esr_far();
    let ec = (esr >> 26) & 0x3f;
    if ec == 0x15 {
        let svc_num = esr & 0xffff;
        // Syscalls run with the caller's interrupt mask, so a long syscall
        // (reading a line, sleeping) can be preempted like any other code.
        let caller_irqs_enabled = frame.spsr & SPSR_I == 0;
        if caller_irqs_enabled { irq::enable(); }
        if !syscall::dispatch_syscall(svc_num, frame) {
            frame.regs[0] = u64::MAX;
        }
        irq::disable();
        return scheduler::after_syscall(frame);
    }
    // A program touching the page below its stack: grow the stack, and
    // try the instruction again. (DFSC 0b0001xx is a translation fault:
    // nothing mapped there.)
    let translation_fault = esr & 0x3c == 0x04;
    if frame.is_user() && ec == 0x24 && translation_fault && crate::process::grow_stack(far as usize) {
        return frame;
    }
    if scheduler::is_user_fault(frame) {
        log::error!("program fault: {} (ec={:#x}) at pc {:#x}, address {:#x}; killing the program", ec_name(ec), ec, frame.elr, far);
        crate::stats::inc(&crate::stats::PROGRAMS_KILLED);
        crate::sound::ui_bonk();
        return scheduler::exit_current(frame);
    }
    log::error!("KERNEL FAULT: {} (ec={:#x}) esr={:#x} pc={:#x} address={:#x} lr={:#x}", ec_name(ec), ec, esr, frame.elr, far, frame.regs[30]);
    crate::klog::dump_for_panic(40);
    if cfg!(feature = "test") {
        crate::test::failed();
    }
    crate::drivers::panic_screen::draw_panic_screen();
    loop { core::hint::spin_loop(); }
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_serror_handler(frame: &mut TrapFrame) {
    let (esr, far) = read_esr_far();
    log::error!("SError esr={:#x} pc={:#x} address={:#x}", esr, frame.elr, far);
    crate::klog::dump_for_panic(40);
    crate::drivers::panic_screen::draw_panic_screen();
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_irq_handler(frame: *mut TrapFrame) -> *mut TrapFrame {
    crate::klog::set_in_irq(true);
    crate::arch::gic::handle_irq();
    crate::klog::set_in_irq(false);
    scheduler::schedule(frame)
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_fiq_handler() {
    log::error!("unexpected FIQ");
    crate::drivers::panic_screen::draw_panic_screen();
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // If this core panicked mid-print, it still holds the console.
    crate::drivers::uart::force_unlock_console();
    println!("Kernel Panic! {}", info);
    crate::klog::dump_for_panic(40);
    if cfg!(feature = "test") {
        crate::test::failed();
    }
    let core_id = crate::arch::psci::get_core_id();
    if core_id != 0 && cfg!(feature = "restart_secondary") { println!("Restarting core {}", core_id); crate::arch::exceptions::restart_core::restart_core(); }
    loop {}
}
