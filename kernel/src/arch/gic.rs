//! GICv2 interrupt controller. Drivers hook interrupts with `register`;
//! every interrupt is delivered to core 0, which runs the scheduler.

use core::ptr::read_volatile;
use core::ptr::write_volatile;
use core::sync::atomic::{AtomicUsize, Ordering};
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

const GICC_CTLR: usize = GICC_BASE + 0x000;
const GICC_PMR: usize = GICC_BASE + 0x004;
const GICC_IAR: usize = GICC_BASE + 0x00C;
const GICC_EOIR: usize = GICC_BASE + 0x010;

/// Interrupt ids we can register handlers for. QEMU virt's devices are all
/// below this: timer 30, UART 33, virtio-mmio slots 48..80.
const MAX_IRQS: usize = 128;
const SPURIOUS: u32 = 1023;

pub const TIMER_IRQ: u32 = 30;
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

pub fn init() {
    unsafe {
        mmio_write(GICD_CTLR, 1);
        mmio_write(GICC_CTLR, 1);
        mmio_write(GICC_PMR, 0xFF);
    }
    register(TIMER_IRQ, crate::arch::timer::on_tick);
    // Nothing to do: the IRQ path runs the scheduler afterwards anyway.
    register(KICK_SGI, || {});
    log::debug!("GICv2 initialized");
}

/// Sets up this core's side of the GIC, on a core other than 0 that runs
/// a scheduler: its CPU interface, and the timer and kick interrupts,
/// whose enable and priority bits each core has its own copy of.
pub fn init_this_core() {
    unsafe {
        mmio_write(GICC_CTLR, 1);
        mmio_write(GICC_PMR, 0xFF);
        for irq in [TIMER_IRQ, KICK_SGI] {
            write_byte_reg(GICD_IPRIORITYR, irq, 0x80);
            mmio_write(GICD_ISENABLER + (irq as usize / 32) * 4, 1 << (irq % 32));
        }
    }
}

/// Interrupts `core` with `KICK_SGI`, so it reschedules: say, because a
/// task of its was just woken.
pub fn kick(core: usize) {
    unsafe {
        // Make what we changed visible before the other core looks.
        core::arch::asm!("dsb ishst");
        mmio_write(GICD_SGIR, (1 << (16 + core)) | KICK_SGI);
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
        write_byte_reg(GICD_IPRIORITYR, irq, 0x80);
        if irq >= 32 {
            // Shared peripheral interrupts: level-triggered, to core 0.
            write_byte_reg(GICD_ITARGETSR, irq, 1);
            let cfg = GICD_ICFGR + (irq as usize / 16) * 4;
            let shift = (irq as usize % 16) * 2;
            mmio_write(cfg, mmio_read(cfg) & !(0b11 << shift));
        }
        mmio_write(GICD_ISENABLER + (irq as usize / 32) * 4, 1 << (irq % 32));
    }
}

pub fn handle_irq() {
    let iar = unsafe { mmio_read(GICC_IAR) };
    let irq = iar & 0x3FF;
    if irq == SPURIOUS {
        return;
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
    unsafe { mmio_write(GICC_EOIR, iar) };
}
