use kos_derive::const_assert;

/// Everything needed to resume a task: saved by the exception vectors
/// (`arch/exceptions/vector.s`) on every syscall, fault and interrupt, and
/// restored on the way out. Offsets are hardcoded in the assembly.
#[repr(C)]
pub struct TrapFrame {
    pub regs: [u64; 31],    // x0..x30          @ 0
    pub elr: u64,           //                  @ 248
    pub spsr: u64,          //                  @ 256
    pub fpcr: u64,          //                  @ 264
    pub fpsr: u64,          //                  @ 272
    /// What the task asked the scheduler for during this syscall (see
    /// `scheduler::Request`). Zeroed on every exception entry.
    pub request: u64,       //                  @ 280
    /// The user stack pointer. Only programs (EL0) use it.
    pub sp_el0: u64,        //                  @ 288
    _pad: u64,              //                  @ 296
    pub q: [u128; 32],      // SIMD/FP q0..q31  @ 304
}

const_assert!(core::mem::size_of::<TrapFrame>() == 816, "TrapFrame layout must match vector.s");
const_assert!(core::mem::offset_of!(TrapFrame, sp_el0) == 288, "TrapFrame layout must match vector.s");
const_assert!(core::mem::offset_of!(TrapFrame, q) == 304, "TrapFrame layout must match vector.s");

/// EL1h (EL1 using SP_EL1), all interrupts unmasked.
const SPSR_EL1H: u64 = 0x5;
/// EL0t: user mode, all interrupts unmasked.
const SPSR_EL0T: u64 = 0x0;
/// SPSR.M\[3:0\], the exception level and stack the frame returns to.
const SPSR_MODE: u64 = 0xf;

impl TrapFrame {
    /// A frame that starts executing `entry`; returning from `entry` jumps to `ret`.
    pub fn new(entry: usize, ret: usize) -> Self {
        let mut regs = [0; 31];
        regs[30] = ret as u64;
        Self { regs, elr: entry as u64, spsr: SPSR_EL1H, fpcr: 0, fpsr: 0, request: 0, sp_el0: 0, _pad: 0, q: [0; 32] }
    }

    /// A frame that starts a program at `entry` in user mode (EL0), on the
    /// user stack `sp`; returning from `entry` jumps to `ret`.
    pub fn new_user(entry: usize, sp: usize, ret: usize) -> Self {
        let mut regs = [0; 31];
        regs[30] = ret as u64;
        Self { regs, elr: entry as u64, spsr: SPSR_EL0T, fpcr: 0, fpsr: 0, request: 0, sp_el0: sp as u64, _pad: 0, q: [0; 32] }
    }

    /// Whether this frame was saved from (and returns to) user mode.
    pub fn is_user(&self) -> bool {
        self.spsr & SPSR_MODE == SPSR_EL0T
    }
}

impl core::ops::Index<usize> for TrapFrame {
    type Output = u64;

    fn index(&self, index: usize) -> &Self::Output {
        &self.regs[index]
    }
}

impl core::ops::IndexMut<usize> for TrapFrame {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.regs[index]
    }
}
