//! The GIC interrupt controller, version 2 or 3. Drivers hook interrupts
//! with `register`; every interrupt is delivered to core 0, which runs the
//! scheduler.
//!
//! QEMU's virt board has a GICv2 by default, but under hvf (macOS's
//! hypervisor) it can only have a GICv3. They share the distributor, which
//! routes device interrupts; a GICv3 swaps the GICv2's memory-mapped CPU
//! interface for system registers, and gives each core a "redistributor"
//! for its own interrupts (the timer, and kicks from other cores).

use core::arch::asm;
use core::ptr::read_volatile;
use core::ptr::write_volatile;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use crate::println;
use crate::mm::layout::phys_to_virt;

const GICD_BASE: usize = phys_to_virt(0x0800_0000);
const GICC_BASE: usize = phys_to_virt(0x0801_0000);

const GICD_CTLR: usize = GICD_BASE + 0x000;
const GICD_ISENABLER: usize = GICD_BASE + 0x100;
const GICD_IPRIORITYR: usize = GICD_BASE + 0x400;
const GICD_ITARGETSR: usize = GICD_BASE + 0x800;
const GICD_ICFGR: usize = GICD_BASE + 0xC00;
const GICD_SGIR: usize = GICD_BASE + 0xF00;
// GICv3 only.
const GICD_IGROUPR: usize = GICD_BASE + 0x080;
const GICD_IROUTER: usize = GICD_BASE + 0x6000;
const GICD_CTLR_RWP: u32 = 1 << 31;

const GICC_CTLR: usize = GICC_BASE + 0x000;
const GICC_PMR: usize = GICC_BASE + 0x004;
const GICC_IAR: usize = GICC_BASE + 0x00C;
const GICC_EOIR: usize = GICC_BASE + 0x010;

// GICv3 redistributors: one per core, each a 64 KiB frame of controls
// followed by a 64 KiB frame for its SGIs and PPIs (interrupts 0-31).
const GICR_BASE: usize = phys_to_virt(0x080A_0000);
const GICR_STRIDE: usize = 0x20000;
const GICR_WAKER: usize = 0x0014;
const GICR_SGI: usize = 0x10000;
const GICR_IGROUPR0: usize = GICR_SGI + 0x080;
const GICR_ISENABLER0: usize = GICR_SGI + 0x100;
const GICR_IPRIORITYR: usize = GICR_SGI + 0x400;

/// Whether this is a GICv3, decided once by `init`.
static V3: AtomicBool = AtomicBool::new(false);

fn is_v3() -> bool {
    V3.load(Ordering::Relaxed)
}

/// Interrupt ids we can register handlers for. QEMU virt's devices are all
/// below this: the virtual timer 27, UART 33, virtio-mmio slots 48..80.
const MAX_IRQS: usize = 128;
const SPURIOUS: u32 = 1023;

pub const TIMER_IRQ: u32 = 27;
/// The software interrupt one core sends another to make it look at its
/// tasks again (`kick`).
pub const KICK_SGI: u32 = 1;
pub const UART_IRQ: u32 = 33;
/// virtio-mmio slot n raises interrupt `VIRTIO_IRQ_BASE + n`.
pub const VIRTIO_IRQ_BASE: u32 = 48;

/// Handler function pointers, indexed by interrupt id (0 = none).
static HANDLERS: [AtomicUsize; MAX_IRQS] = [const { AtomicUsize::new(0) }; MAX_IRQS];

unsafe fn mmio_write(addr: usize, val: u32) { unsafe { write_volatile(addr as *mut u32, val); } }
unsafe fn mmio_read(addr: usize) -> u32 { unsafe { read_volatile(addr as *const u32) } }

/// Sets one byte of a byte-per-interrupt register bank.
unsafe fn write_byte_reg(bank: usize, irq: u32, value: u32) {
    let reg = bank + (irq as usize / 4) * 4;
    let shift = (irq as usize % 4) * 8;
    let cur = mmio_read(reg);
    mmio_write(reg, cur & !(0xFF << shift) | (value << shift));
}

fn this_core() -> usize {
    let mpidr: u64;
    unsafe { asm!("mrs {}, mpidr_el1", out(reg) mpidr, options(nomem, nostack)) };
    (mpidr & 0xFF) as usize
}

/// This core's redistributor (GICv3).
fn redistributor() -> usize {
    GICR_BASE + this_core() * GICR_STRIDE
}

pub fn init() {
    // The distributor's ID register PIDR2 has the version in bits 4-7. A
    // GICv2's is at 0xFE8; a GICv3's is at 0xFFE8, past the end of a
    // GICv2's registers, so it's only read once 0xFE8 says it's not a
    // GICv2. (The CPU's ID_AA64PFR0_EL1 can't be trusted for this: under
    // hvf it says there's no GICv3 when there is.)
    unsafe {
        let v2 = (mmio_read(GICD_BASE + 0xFE8) >> 4) & 0xF == 2;
        V3.store(!v2 && (mmio_read(GICD_BASE + 0xFFE8) >> 4) & 0xF >= 3, Ordering::Relaxed);
    }
    unsafe {
        if is_v3() {
            // Affinity routing on, both groups of interrupts on (with one
            // security state, as here, group 1 is what reaches the kernel).
            mmio_write(GICD_CTLR, (1 << 4) | (1 << 1) | 1);
            while mmio_read(GICD_CTLR) & GICD_CTLR_RWP != 0 {}
        } else {
            mmio_write(GICD_CTLR, 1);
        }
    }
    init_cpu_interface();
    register(TIMER_IRQ, crate::arch::timer::on_tick);
    // Nothing to do: the IRQ path runs the scheduler afterwards anyway.
    register(KICK_SGI, || {});
    log::info!("interrupts: GICv{}", if is_v3() { 3 } else { 2 });
}

/// Turns on this core's CPU interface, so interrupts reach it.
fn init_cpu_interface() {
    unsafe {
        if is_v3() {
            // Wake the redistributor, then switch on the system registers
            // (ICC_SRE_EL1), let every priority through (ICC_PMR_EL1) and
            // enable group 1 (ICC_IGRPEN1_EL1).
            let rd = redistributor();
            mmio_write(rd + GICR_WAKER, mmio_read(rd + GICR_WAKER) & !(1 << 1));
            while mmio_read(rd + GICR_WAKER) & (1 << 2) != 0 {}
            mmio_write(rd + GICR_IGROUPR0, 0xFFFF_FFFF);
            asm!("msr s3_0_c12_c12_5, {}", "isb", in(reg) 1u64);
            asm!("msr s3_0_c4_c6_0, {}", in(reg) 0xFFu64);
            asm!("msr s3_0_c12_c12_7, {}", "isb", in(reg) 1u64);
        } else {
            mmio_write(GICC_CTLR, 1);
            mmio_write(GICC_PMR, 0xFF);
        }
    }
}

/// Enables one of this core's own interrupts (0-31: SGIs and PPIs), which
/// each core has its own enable and priority bits for.
unsafe fn enable_private(irq: u32) {
    unsafe {
        if is_v3() {
            let rd = redistributor();
            write_byte_reg(rd + GICR_IPRIORITYR, irq, 0x80);
            mmio_write(rd + GICR_ISENABLER0, 1 << irq);
        } else {
            write_byte_reg(GICD_IPRIORITYR, irq, 0x80);
            mmio_write(GICD_ISENABLER, 1 << irq);
        }
    }
}

/// Sets up this core's side of the GIC, on a core other than 0 that runs
/// a scheduler: its CPU interface, and the timer and kick interrupts.
pub fn init_this_core() {
    init_cpu_interface();
    unsafe {
        for irq in [TIMER_IRQ, KICK_SGI] {
            enable_private(irq);
        }
    }
}

/// Interrupts `core` with `KICK_SGI`, so it reschedules: say, because a
/// task of its was just woken.
pub fn kick(core: usize) {
    unsafe {
        // Make what we changed visible before the other core looks.
        asm!("dsb ishst");
        if is_v3() {
            // ICC_SGI1R_EL1: the interrupt, and the target cores as a
            // bitmap of affinity-0 numbers (all of QEMU's cores have
            // affinity 0.0.0.n).
            asm!("msr s3_0_c12_c11_5, {}", "isb", in(reg) ((KICK_SGI as u64) << 24) | (1u64 << core));
        } else {
            mmio_write(GICD_SGIR, (1 << (16 + core)) | KICK_SGI);
        }
    }
}

/// Routes interrupt `irq` to `handler` on core 0 and enables it. Handlers
/// run in IRQ context: keep them short, and only wake tasks / touch
/// lock-free or `IrqMutex` state.
pub fn register(irq: u32, handler: fn()) {
    assert!((irq as usize) < MAX_IRQS, "irq {} out of range", irq);
    HANDLERS[irq as usize].store(handler as usize, Ordering::Release);
    log::debug!("irq {} registered", irq);
    unsafe {
        if irq < 32 {
            enable_private(irq);
            return;
        }
        // Shared peripheral interrupts: level-triggered, to core 0.
        write_byte_reg(GICD_IPRIORITYR, irq, 0x80);
        if is_v3() {
            let group = GICD_IGROUPR + (irq as usize / 32) * 4;
            mmio_write(group, mmio_read(group) | 1 << (irq % 32));
            write_volatile((GICD_IROUTER + irq as usize * 8) as *mut u64, 0);
        } else {
            write_byte_reg(GICD_ITARGETSR, irq, 1);
        }
        let cfg = GICD_ICFGR + (irq as usize / 16) * 4;
        let shift = (irq as usize % 16) * 2;
        mmio_write(cfg, mmio_read(cfg) & !(0b11 << shift));
        mmio_write(GICD_ISENABLER + (irq as usize / 32) * 4, 1 << (irq % 32));
    }
}

/// Takes the interrupt being raised, from ICC_IAR1_EL1 or GICC_IAR.
fn acknowledge() -> u32 {
    if is_v3() {
        let iar: u64;
        unsafe { asm!("mrs {}, s3_0_c12_c12_0", out(reg) iar, options(nomem, nostack)) };
        iar as u32
    } else {
        unsafe { mmio_read(GICC_IAR) }
    }
}

fn end_of_interrupt(iar: u32) {
    unsafe {
        if is_v3() {
            asm!("msr s3_0_c12_c12_1, {}", "isb", in(reg) iar as u64);
        } else {
            mmio_write(GICC_EOIR, iar);
        }
    }
}

pub fn handle_irq() {
    let iar = acknowledge();
    // A GICv2 puts the sending core's number above the interrupt id (bits
    // 10-12) for SGIs; a GICv3 uses all 24 bits for the id.
    let irq = iar & if is_v3() { 0xFF_FFFF } else { 0x3FF };
    if irq >= SPURIOUS - 3 {
        return;  // 1020-1023: nothing to do after all
    }
    if let Some(counter) = crate::stats::IRQS.get(irq as usize) {
        crate::stats::inc(counter);
    }
    if irq != TIMER_IRQ {
        log::trace!("irq {}", irq);
    }
    let handler = HANDLERS.get(irq as usize).map_or(0, |h| h.load(Ordering::Acquire));
    if handler != 0 {
        let handler: fn() = unsafe { core::mem::transmute(handler) };
        handler();
    } else {
        log::warn!("unhandled irq {}", irq);
    }
    end_of_interrupt(iar);
}
