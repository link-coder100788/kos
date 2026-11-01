//! Shared virtio-mmio (modern, version 2) plumbing: device discovery, the
//! init handshake, and a split virtqueue in frame-allocated memory.

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{fence, Ordering};
use crate::mm::frame;
use crate::mm::layout::{phys_to_virt, PAGE_SIZE};

pub const MMIO_BASE: usize = phys_to_virt(0x0a00_0000);
pub const MMIO_STRIDE: usize = 0x200;
pub const MMIO_SLOTS: usize = 32;
const MAGIC: u32 = 0x7472_6976;

pub mod reg {
    pub const MAGIC_VALUE: usize = 0x000;
    pub const VERSION: usize = 0x004;
    pub const DEVICE_ID: usize = 0x008;
    pub const DEVICE_FEATURES: usize = 0x010;
    pub const DEVICE_FEATURES_SEL: usize = 0x014;
    pub const DRIVER_FEATURES: usize = 0x020;
    pub const DRIVER_FEATURES_SEL: usize = 0x024;
    pub const QUEUE_SEL: usize = 0x030;
    pub const QUEUE_NUM_MAX: usize = 0x034;
    pub const QUEUE_NUM: usize = 0x038;
    pub const QUEUE_READY: usize = 0x044;
    pub const QUEUE_NOTIFY: usize = 0x050;
    pub const INTERRUPT_STATUS: usize = 0x060;
    pub const INTERRUPT_ACK: usize = 0x064;
    pub const STATUS: usize = 0x070;
    pub const QUEUE_DESC_LOW: usize = 0x080;
    pub const QUEUE_DESC_HIGH: usize = 0x084;
    pub const QUEUE_DRIVER_LOW: usize = 0x090;
    pub const QUEUE_DRIVER_HIGH: usize = 0x094;
    pub const QUEUE_DEVICE_LOW: usize = 0x0a0;
    pub const QUEUE_DEVICE_HIGH: usize = 0x0a4;
    pub const CONFIG: usize = 0x100;
}

const STATUS_ACKNOWLEDGE: u32 = 1;
const STATUS_DRIVER: u32 = 2;
const STATUS_DRIVER_OK: u32 = 4;
const STATUS_FEATURES_OK: u32 = 8;
const STATUS_FAILED: u32 = 128;
/// VIRTIO_F_VERSION_1, bit 32 of the feature set.
const FEATURE_HI_VERSION_1: u32 = 1 << 0;

pub const DESC_F_NEXT: u16 = 1;
pub const DESC_F_WRITE: u16 = 2;

/// A virtio-mmio device: just its register window.
#[derive(Clone, Copy)]
pub struct Device {
    pub base: usize,
}

impl Device {
    pub fn read(&self, off: usize) -> u32 {
        unsafe { read_volatile((self.base + off) as *const u32) }
    }

    pub fn write(&self, off: usize, val: u32) {
        unsafe { write_volatile((self.base + off) as *mut u32, val) }
    }

    pub fn read_config_u8(&self, off: usize) -> u8 {
        unsafe { read_volatile((self.base + reg::CONFIG + off) as *const u8) }
    }

    pub fn write_config_u8(&self, off: usize, val: u8) {
        unsafe { write_volatile((self.base + reg::CONFIG + off) as *mut u8, val) }
    }

    /// The slot index, i.e. which interrupt it raises (see gic::VIRTIO_IRQ_BASE).
    pub fn slot(&self) -> u32 {
        ((self.base - MMIO_BASE) / MMIO_STRIDE) as u32
    }

    /// Resets the device and negotiates features: only VERSION_1, plus
    /// whatever of `wanted_lo` (bits 0..32) the device offers.
    pub fn begin_init(&self, wanted_lo: u32) -> bool {
        self.write(reg::STATUS, 0);
        self.write(reg::STATUS, STATUS_ACKNOWLEDGE);
        self.write(reg::STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER);
        self.write(reg::DEVICE_FEATURES_SEL, 0);
        let lo = self.read(reg::DEVICE_FEATURES) & wanted_lo;
        self.write(reg::DEVICE_FEATURES_SEL, 1);
        let hi = self.read(reg::DEVICE_FEATURES) & FEATURE_HI_VERSION_1;
        self.write(reg::DRIVER_FEATURES_SEL, 0);
        self.write(reg::DRIVER_FEATURES, lo);
        self.write(reg::DRIVER_FEATURES_SEL, 1);
        self.write(reg::DRIVER_FEATURES, hi);
        self.write(reg::STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK);
        if self.read(reg::STATUS) & STATUS_FEATURES_OK == 0 {
            self.write(reg::STATUS, STATUS_FAILED);
            return false;
        }
        true
    }

    /// Tells the device the driver is ready; call after setting up queues.
    pub fn finish_init(&self) {
        self.write(reg::STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK);
    }

    /// Acknowledges a pending interrupt.
    pub fn ack_interrupt(&self) {
        let status = self.read(reg::INTERRUPT_STATUS);
        self.write(reg::INTERRUPT_ACK, status);
    }
}

/// Every modern virtio-mmio device with the given device id.
pub fn find_devices(device_id: u32) -> impl Iterator<Item = Device> {
    (0..MMIO_SLOTS).map(|slot| Device { base: MMIO_BASE + slot * MMIO_STRIDE }).filter(move |dev| {
        dev.read(reg::MAGIC_VALUE) == MAGIC && dev.read(reg::VERSION) == 2 && dev.read(reg::DEVICE_ID) == device_id
    })
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Desc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

/// A split virtqueue of `size` entries in one page: descriptors at 0,
/// available ring at 1 KiB, used ring at 2 KiB (enough for 64 entries).
pub struct VirtQueue {
    ring: usize,
    size: u16,
    last_used: u16,
}

const AVAIL_OFF: usize = 1024;
const USED_OFF: usize = 2048;
pub const MAX_QUEUE_SIZE: u16 = 64;

impl VirtQueue {
    /// Allocates queue `index` of `dev` and hands it to the device.
    pub fn new(dev: &Device, index: u32, size: u16) -> Option<Self> {
        assert!(size <= MAX_QUEUE_SIZE);
        dev.write(reg::QUEUE_SEL, index);
        if dev.read(reg::QUEUE_NUM_MAX) < size as u32 {
            return None;
        }
        let phys = frame::alloc_zeroed()?;
        dev.write(reg::QUEUE_NUM, size as u32);
        let set = |lo: usize, hi: usize, addr: usize| {
            dev.write(lo, addr as u32);
            dev.write(hi, (addr >> 32) as u32);
        };
        set(reg::QUEUE_DESC_LOW, reg::QUEUE_DESC_HIGH, phys);
        set(reg::QUEUE_DRIVER_LOW, reg::QUEUE_DRIVER_HIGH, phys + AVAIL_OFF);
        set(reg::QUEUE_DEVICE_LOW, reg::QUEUE_DEVICE_HIGH, phys + USED_OFF);
        dev.write(reg::QUEUE_READY, 1);
        Some(Self { ring: phys_to_virt(phys), size, last_used: 0 })
    }

    /// Sets descriptor `i` to a single device-writable buffer (physical address).
    pub fn set_writable_buffer(&mut self, i: u16, phys: usize, len: u32) {
        self.set_buffer(i, phys, len, DESC_F_WRITE, 0);
    }

    /// Sets descriptor `i` to any buffer: `flags` is DESC_F_* and `next`
    /// the following descriptor of a chain (with DESC_F_NEXT).
    pub fn set_buffer(&mut self, i: u16, phys: usize, len: u32, flags: u16, next: u16) {
        let desc = (self.ring as *mut Desc).wrapping_add(i as usize);
        unsafe { write_volatile(desc, Desc { addr: phys as u64, len, flags, next }) };
    }

    /// Offers descriptor `i` to the device. Call `notify` afterwards.
    pub fn make_available(&mut self, i: u16) {
        let idx_ptr = (self.ring + AVAIL_OFF + 2) as *mut u16;
        unsafe {
            let idx = read_volatile(idx_ptr);
            let slot = (self.ring + AVAIL_OFF + 4) as *mut u16;
            write_volatile(slot.add((idx % self.size) as usize), i);
            fence(Ordering::SeqCst);
            write_volatile(idx_ptr, idx.wrapping_add(1));
            fence(Ordering::SeqCst);
        }
    }

    pub fn notify(&self, dev: &Device, index: u32) {
        dev.write(reg::QUEUE_NOTIFY, index);
    }

    /// The next descriptor the device has finished with, and how many
    /// bytes it wrote.
    pub fn pop_used(&mut self) -> Option<(u16, u32)> {
        unsafe {
            let used_idx = read_volatile((self.ring + USED_OFF + 2) as *const u16);
            if used_idx == self.last_used {
                return None;
            }
            fence(Ordering::SeqCst);
            let elem = (self.ring + USED_OFF + 4 + (self.last_used % self.size) as usize * 8) as *const u32;
            let id = read_volatile(elem) as u16;
            let len = read_volatile(elem.add(1));
            self.last_used = self.last_used.wrapping_add(1);
            Some((id, len))
        }
    }
}

const _: () = assert!(USED_OFF + 4 + 8 * MAX_QUEUE_SIZE as usize + 2 <= PAGE_SIZE);
