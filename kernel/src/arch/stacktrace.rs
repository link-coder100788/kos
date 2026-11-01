use core::fmt;
use core::arch::asm;

#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct RegisterDump {
    pub x: [u64; 30],
    pub lr: u64,
    pub sp: u64,
    pub elr_el1: u64, 
    pub spsr_el1: u64, 
    pub esr_el1: u64, 
    pub far_el1: u64, 
    pub ttbr0_el1: u64, 
    pub ttbr1_el1: u64, 
    pub ticks: u64,
}

impl RegisterDump {
    #[inline(always)]
    pub fn capture() -> Self {
        let mut dump = Self {
            x: [0; 30],
            lr: 0,
            sp: 0,
            elr_el1: 0,
            spsr_el1: 0,
            esr_el1: 0,
            far_el1: 0,
            ttbr0_el1: 0,
            ttbr1_el1: 0,
            ticks: 0,
        };
        unsafe {
            asm!(
                "str x0,  [{0}, #0]",
                "str x1,  [{0}, #8]",
                "str x2,  [{0}, #16]",
                "str x3,  [{0}, #24]",
                "str x4,  [{0}, #32]",
                "str x5,  [{0}, #40]",
                "str x6,  [{0}, #48]",
                "str x7,  [{0}, #56]",
                "str x8,  [{0}, #64]",
                "str x9,  [{0}, #72]",
                "str x10, [{0}, #80]",
                "str x11, [{0}, #88]",
                "str x12, [{0}, #96]",
                "str x13, [{0}, #104]",
                "str x14, [{0}, #112]",
                "str x15, [{0}, #120]",
                "str x16, [{0}, #128]",
                "str x17, [{0}, #136]",
                "str x18, [{0}, #144]",
                "str x19, [{0}, #152]",
                "str x20, [{0}, #160]",
                "str x21, [{0}, #168]",
                "str x22, [{0}, #176]",
                "str x23, [{0}, #184]",
                "str x24, [{0}, #192]",
                "str x25, [{0}, #200]",
                "str x26, [{0}, #208]",
                "str x27, [{0}, #216]",
                "str x28, [{0}, #224]",
                "str x29, [{0}, #232]",
                in(reg) &mut dump.x,
            );
            asm!(
                "mov {0}, x30",
                "mov {1}, sp",
                out(reg) dump.lr,
                out(reg) dump.sp,
            );
            asm!(
                "mrs {0}, elr_el1",
                "mrs {1}, spsr_el1",
                "mrs {2}, esr_el1",
                "mrs {3}, far_el1",
                "mrs {4}, ttbr0_el1",
                "mrs {5}, ttbr1_el1",
                out(reg) dump.elr_el1,
                out(reg) dump.spsr_el1,
                out(reg) dump.esr_el1,
                out(reg) dump.far_el1,
                out(reg) dump.ttbr0_el1,
                out(reg) dump.ttbr1_el1,
            );
        }
        dump.ticks = crate::arch::timer::TICKS.load(core::sync::atomic::Ordering::Relaxed);
        dump
    }
}

impl core::fmt::Display for RegisterDump {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let id: u64; unsafe { asm!("mrs {}, mpidr_el1", out(reg) id) };
        writeln!(f, "====================== REGISTER DUMP ======================")?;
        writeln!(f, "[ General Purpose Registers ]")?;
        for i in (0..30).step_by(2) {
            writeln!(
                f,
                "x{:<2}: {:#018x} {:<20}   x{:<2}: {:#018x} {}",
                i, self.x[i], self.x[i], i + 1, self.x[i + 1], self.x[i + 1]
            )?;
        }
        writeln!(f, "LR : {:#018x}                        SP : {:#018x}", self.lr, self.sp)?;
        writeln!(f, "")?;
        writeln!(f, "[ System & Control Registers (EL1) ]")?;
        writeln!(f, "ELR : {:#018x}    SPSR: {:#018x}", self.elr_el1, self.spsr_el1)?;
        writeln!(f, "ESR : {:#018x}    FAR : {:#018x}", self.esr_el1, self.far_el1)?;
        writeln!(f, "TTB0: {:#018x}    TTB1: {:#018x}", self.ttbr0_el1, self.ttbr1_el1)?;
        writeln!(f, "")?;
        writeln!(f, "Ticks: {}    CPU: {}", self.ticks, id & 0xff)?;
        writeln!(f, "===================================================================")
    }
}