//! Masking IRQs on the current core.

use core::arch::asm;

/// The I (IRQ mask) bit in DAIF / SPSR.
pub const SPSR_I: u64 = 1 << 7;

pub fn enable() {
    unsafe { asm!("msr daifclr, #2", options(nostack)) };
}

pub fn disable() {
    unsafe { asm!("msr daifset, #2", options(nostack)) };
}

/// Runs `f` with IRQs masked on this core, restoring the previous mask after.
pub fn without_irqs<R>(f: impl FnOnce() -> R) -> R {
    let daif: u64;
    unsafe { asm!("mrs {0}, daif", out(reg) daif, options(nostack)) };
    disable();
    let result = f();
    if daif & SPSR_I == 0 {
        enable();
    }
    result
}
