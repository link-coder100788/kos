use core::ptr::read_volatile;
use core::ptr::write_volatile;
use core::ptr::addr_of;
use core::ptr::addr_of_mut;
use crate::println;
use crate::scheduler::wait::WaitQueue;
use core::sync::atomic::{AtomicUsize, Ordering};
use crate::mm::layout::{phys_to_virt, virt_to_phys};

/// The initialized device's MMIO base, for the interrupt handler.
static BLK_BASE: AtomicUsize = AtomicUsize::new(0);
/// Woken when the device completes a request.
static BLK_WAIT: WaitQueue = WaitQueue::new();

fn blk_irq() {
    let base = BLK_BASE.load(Ordering::Acquire);
    let status = mmio_read(base, reg::INTERRUPT_STATUS);
    mmio_write(base, reg::INTERRUPT_ACK, status);
    BLK_WAIT.wake_all();
}

/// Physical address of a kernel object, for the device.
fn dma<T>(p: *const T) -> u64 {
    virt_to_phys(p as usize) as u64
}

const VIRTIO_MMIO_BASE: usize = phys_to_virt(0x0a00_0000);
const VIRTIO_MMIO_STRIDE: usize = 0x200;
const VIRTIO_MMIO_SLOTS: usize = 32;

const MAGIC: u32 = 0x7472_6976;
const BLOCK_DEVICE_ID: u32 = 2;

mod reg {
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

const VIRTIO_F_VERSION_1: u32 = 1 << 0;

fn mmio_write(base: usize, off: usize, val: u32) {
    unsafe { write_volatile((base + off) as *mut u32, val); }
}

fn mmio_read(base: usize, off: usize) -> u32 {
    unsafe { read_volatile((base + off) as *const u32) }
}

pub fn find_blk_device() -> Option<usize> {
    for slot in 0..VIRTIO_MMIO_SLOTS {
        let base = VIRTIO_MMIO_BASE + slot * VIRTIO_MMIO_STRIDE;
        let magic = mmio_read(base, reg::MAGIC_VALUE);
        if magic != MAGIC {
            continue;
        }
        let version = mmio_read(base, reg::VERSION);
        let device_id = mmio_read(base, reg::DEVICE_ID);
        if device_id != 0 {
            log::debug!("virtio-mmio slot {}: version={} device_id={}", slot, version, device_id);
        }
        if device_id == BLOCK_DEVICE_ID {
            if version != 2 {
                // This driver only implements the modern (v2) virtio-mmio
                // register layout (QUEUE_DESC/DRIVER/DEVICE + QUEUE_READY).
                // A version=1 legacy device uses GuestPageSize/QueuePFN
                // instead and will silently fail to receive queue addresses
                // if we proceed, causing the device to never see our rings.
                // Fix: pass `disable-legacy=on` on the QEMU -device line.
                log::warn!(
                    "virtio-blk: slot {} is legacy (version={}), this driver requires version=2. \
                     Add disable-legacy=on to the QEMU -device virtio-blk-device line.",
                    slot, version
                );
                continue;
            }
            return Some(base);
        }
    }
    None
}

const QUEUE_SIZE: usize = 8;

#[repr(C)]
#[derive(Clone, Copy)]
struct Desc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

const VIRTQ_DESC_F_NEXT: u16 = 1;
const VIRTQ_DESC_F_WRITE: u16 = 2;

#[repr(C, align(4096))]
struct DescTable([Desc; QUEUE_SIZE]);

#[repr(C, align(4096))]
struct AvailRing {
    flags: u16,
    idx: u16,
    ring: [u16; QUEUE_SIZE],
    used_event: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct UsedElem { id: u32, len: u32 }

#[repr(C, align(4096))]
struct UsedRing {
    flags: u16,
    idx: u16,
    ring: [UsedElem; QUEUE_SIZE],
    avail_event: u16,
}

static mut DESC_TABLE: DescTable = unsafe { core::mem::zeroed() };
static mut AVAIL: AvailRing = unsafe { core::mem::zeroed() };
static mut USED: UsedRing = unsafe { core::mem::zeroed() };
static mut LAST_USED_IDX: u16 = 0;

// ---- Volatile accessors -----------------------------------------------
// Every field of DESC_TABLE / AVAIL / USED that is written by us and read
// by the device, or written by the device and read by us, MUST go through
// one of these. No plain field access on these statics anywhere else in
// the file. This is what actually prevents LLVM from hoisting/eliding the
// polling loop, unlike a fence alone.

unsafe fn set_desc(i: usize, d: Desc) {
    write_volatile(addr_of_mut!(DESC_TABLE.0[i]), d);
}

unsafe fn set_avail_ring(i: usize, val: u16) {
    write_volatile(addr_of_mut!(AVAIL.ring[i]), val);
}

unsafe fn read_avail_idx() -> u16 {
    read_volatile(addr_of!(AVAIL.idx))
}

unsafe fn write_avail_idx(val: u16) {
    write_volatile(addr_of_mut!(AVAIL.idx), val);
}

unsafe fn read_used_idx() -> u16 {
    read_volatile(addr_of!(USED.idx))
}

unsafe fn read_last_used_idx() -> u16 {
    read_volatile(addr_of!(LAST_USED_IDX))
}

unsafe fn write_last_used_idx(val: u16) {
    write_volatile(addr_of_mut!(LAST_USED_IDX), val);
}

const VIRTIO_BLK_T_IN: u32 = 0;  // read
const VIRTIO_BLK_T_OUT: u32 = 1; // write

const VIRTIO_BLK_F_RO: u32 = 1 << 5; // device is read-only

#[repr(C)]
#[derive(Clone, Copy)]
struct BlkReqHeader {
    req_type: u32,
    reserved: u32,
    sector: u64,
}

pub struct BlkDevice {
    base: usize,
    read_only: bool,
}

impl BlkDevice {
    pub fn init() -> Option<Self> {
        let base = find_blk_device()?;
        mmio_write(base, reg::STATUS, 0);
        mmio_write(base, reg::STATUS, STATUS_ACKNOWLEDGE);
        mmio_write(base, reg::STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER);
        mmio_write(base, reg::DEVICE_FEATURES_SEL, 0);
        let features_lo = mmio_read(base, reg::DEVICE_FEATURES);
        let read_only = features_lo & VIRTIO_BLK_F_RO != 0;
        if read_only {
            log::warn!("device is read-only; writes will fail");
        }
        mmio_write(base, reg::DEVICE_FEATURES_SEL, 1);
        let features_hi = mmio_read(base, reg::DEVICE_FEATURES);
        mmio_write(base, reg::DRIVER_FEATURES_SEL, 1);
        mmio_write(base, reg::DRIVER_FEATURES, features_hi & VIRTIO_F_VERSION_1);
        mmio_write(base, reg::DEVICE_FEATURES_SEL, 0);
        mmio_write(base, reg::DRIVER_FEATURES_SEL, 0);
        mmio_write(base, reg::DRIVER_FEATURES, 0);
        mmio_write(base, reg::STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK);
        let status = mmio_read(base, reg::STATUS);
        if status & STATUS_FEATURES_OK == 0 {
            log::error!("device rejected our feature set");
            mmio_write(base, reg::STATUS, STATUS_FAILED);
            return None;
        }
        mmio_write(base, reg::QUEUE_SEL, 0);
        let max = mmio_read(base, reg::QUEUE_NUM_MAX);
        if max == 0 {
            log::error!("queue 0 unavailable");
            return None;
        }
        if (QUEUE_SIZE as u32) > max {
            log::error!("queue size {} exceeds max {}", QUEUE_SIZE, max);
            return None;
        }
        mmio_write(base, reg::QUEUE_NUM, QUEUE_SIZE as u32);
        unsafe {
            let desc_addr = dma(addr_of!(DESC_TABLE));
            let avail_addr = dma(addr_of!(AVAIL));
            let used_addr = dma(addr_of!(USED));
            mmio_write(base, reg::QUEUE_DESC_LOW, desc_addr as u32);
            mmio_write(base, reg::QUEUE_DESC_HIGH, (desc_addr >> 32) as u32);
            mmio_write(base, reg::QUEUE_DRIVER_LOW, avail_addr as u32);
            mmio_write(base, reg::QUEUE_DRIVER_HIGH, (avail_addr >> 32) as u32);
            mmio_write(base, reg::QUEUE_DEVICE_LOW, used_addr as u32);
            mmio_write(base, reg::QUEUE_DEVICE_HIGH, (used_addr >> 32) as u32);
        }
        mmio_write(base, reg::QUEUE_READY, 1);
        BLK_BASE.store(base, Ordering::Release);
        let slot = ((base - VIRTIO_MMIO_BASE) / VIRTIO_MMIO_STRIDE) as u32;
        crate::arch::gic::register(crate::arch::gic::VIRTIO_IRQ_BASE + slot, blk_irq);
        mmio_write(base, reg::STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK);
        log::info!("disk at slot {}: {} MiB, queue size {}", slot, BlkDevice { base, read_only }.capacity_sectors() / 2048, QUEUE_SIZE);
        Some(BlkDevice { base, read_only })
    }

    /// Submits a single request (header + one data buffer + status byte)
    /// and blocks until the device completes it.
    ///
    /// `data_addr` is the physical address of the 512-byte
    /// data buffer. `data_device_writable` must be true for reads (the
    /// device writes into our buffer) and false for writes (the device
    /// only reads from our buffer).
    ///
    /// Safety / concurrency: HEADER, STATUS_BYTE, and the shared queue
    /// statics are not synchronized. Do not call this concurrently from
    /// multiple contexts (e.g. from an interrupt handler and a normal
    /// call path, or from two threads) without adding locking.
    unsafe fn submit_request(&self, req_type: u32, sector: u64, data_addr: u64, data_device_writable: bool) -> bool {
        static mut HEADER: BlkReqHeader = BlkReqHeader { req_type: 0, reserved: 0, sector: 0 };
        static mut STATUS_BYTE: u8 = 0xff;

        let header = BlkReqHeader { req_type, reserved: 0, sector };
        write_volatile(addr_of_mut!(HEADER), header);
        write_volatile(addr_of_mut!(STATUS_BYTE), 0xff);

        set_desc(0, Desc {
            addr: dma(addr_of!(HEADER)),
            len: core::mem::size_of::<BlkReqHeader>() as u32,
            flags: VIRTQ_DESC_F_NEXT,
            next: 1,
        });

        let data_flags = if data_device_writable {
            VIRTQ_DESC_F_NEXT | VIRTQ_DESC_F_WRITE
        } else {
            VIRTQ_DESC_F_NEXT
        };
        set_desc(1, Desc {
            addr: data_addr,
            len: 512,
            flags: data_flags,
            next: 2,
        });

        set_desc(2, Desc {
            addr: dma(addr_of!(STATUS_BYTE)),
            len: 1,
            flags: VIRTQ_DESC_F_WRITE,
            next: 0,
        });

        let avail_idx = read_avail_idx();
        set_avail_ring((avail_idx % QUEUE_SIZE as u16) as usize, 0);

        // fence: make sure the descriptor + ring write are visible
        // before we publish the new avail idx
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        write_avail_idx(avail_idx.wrapping_add(1));
        // fence: make sure avail idx write is visible before notifying
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        mmio_write(self.base, reg::QUEUE_NOTIFY, 0);

        let started = crate::arch::timer::uptime_ms();
        let deadline = crate::arch::timer::deadline_after_ms(5000);
        let done = BLK_WAIT.wait_until_deadline(Some(deadline), || read_used_idx() != read_last_used_idx());
        if !done {
            log::error!("request timed out (type={}, sector={})", req_type, sector);
            return false;
        }

        write_last_used_idx(read_last_used_idx().wrapping_add(1));
        let ok = read_volatile(addr_of!(STATUS_BYTE)) == 0;
        crate::stats::inc(if req_type == VIRTIO_BLK_T_IN { &crate::stats::DISK_READS } else { &crate::stats::DISK_WRITES });
        log::trace!("{} sector {} {} ({} ms)", if req_type == VIRTIO_BLK_T_IN { "read" } else { "write" }, sector,
            if ok { "ok" } else { "FAILED" }, crate::arch::timer::uptime_ms() - started);
        if !ok {
            log::error!("disk {} of sector {} failed", if req_type == VIRTIO_BLK_T_IN { "read" } else { "write" }, sector);
        }
        ok
    }

    pub fn read_sector(&self, sector: u64, buf: &mut [u8; 512]) -> bool {
        unsafe { self.submit_request(VIRTIO_BLK_T_IN, sector, dma(buf.as_ptr()), true) }
    }

    pub fn write_sector(&self, sector: u64, buf: &[u8; 512]) -> bool {
        if self.read_only {
            log::warn!("refusing write to sector {}: device is read-only", sector);
            return false;
        }
        unsafe { self.submit_request(VIRTIO_BLK_T_OUT, sector, dma(buf.as_ptr()), false) }
    }

    pub fn capacity_sectors(&self) -> u64 {
        let lo = mmio_read(self.base, reg::CONFIG) as u64;
        let hi = mmio_read(self.base, reg::CONFIG + 4) as u64;
        lo | (hi << 32)
    }
}

pub extern "C" fn drive_task() -> ! {
    let blk = match BlkDevice::init() {
        Some(dev) => dev,
        None => {
            println!("virtio-blk: failed to initialize");
            loop {}
        }
    };
    let mut buf = [0u8; 512];
    for (i, b) in buf.iter_mut().enumerate() { *b = i as u8; }
    if blk.write_sector(0, &buf) {
        println!("write ok");
        let mut rbuf = [0u8; 512];
        if blk.read_sector(0, &mut rbuf) {
            if rbuf == buf {
                println!("read ok, data matches");
            } else {
                println!("read ok, data does not match");
            }
        }
    }
    loop {}
}
