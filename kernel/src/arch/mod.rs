//! AArch64-specific code: exception vectors, interrupt controller, timer,
//! PSCI, and the secondary-core entry points.

core::arch::global_asm!(include_str!("boot.s"));

pub mod exceptions;
pub mod gic;
pub mod irq;
pub mod mmu;
pub mod psci;
pub mod smp;
pub mod stacktrace;
pub mod timer;
