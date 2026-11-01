use crate::mm::layout::virt_to_phys;

unsafe extern "C" {
    fn restart_trampoline() -> !;
}

/// Restarts this (secondary) core: jumps to `restart_trampoline` at its
/// physical (identity-mapped) address, which turns the MMU off and
/// re-enters the bootloader, which sets the core's stack up again and
/// sends it back through its boot stub.
pub fn restart_core() -> ! {
    let trampoline = virt_to_phys(restart_trampoline as usize);
    unsafe { core::arch::asm!("msr daifset, #0xf", "br {0}", in(reg) trampoline, options(noreturn)) }
}
