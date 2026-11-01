//! virtio-net (device ID 1) driver over virtio-mmio, **modern** (non-legacy)
//! transport, for QEMU `-machine virt`.
//!
//! Assumes the device is exposed the same way as your virtio-blk device:
//! `-global virtio-mmio.force-legacy=false`, i.e. MMIO `Version` register
//! reads `2`.
//!
//! Implements `smoltcp::phy::Device` directly (current GAT-based trait:
//! `type RxToken<'a>` / `type TxToken<'a>`, `RxToken::consume(self, f)`,
//! `TxToken::consume(self, len, f)`).
//!
//! ## What this does NOT do (by design, see notes at bottom)
//! - No control virtqueue (no MAC filtering / multiqueue / RSS).
//! - No checksum/GSO offload (VIRTIO_NET_F_CSUM/GUEST_CSUM not negotiated) —
//!   you negotiated MAC + STATUS only, so smoltcp computes checksums in
//!   software, which is correct and simplest.
//! - No VIRTIO_RING_F_EVENT_IDX — device interrupts on every completed
//!   descriptor. Fine to start; an easy later optimization.
//!
//! ## Integration model
//! `Device::receive`/`transmit` are what smoltcp calls every time you poll
//! the `Interface` — they are synchronous, non-blocking checks against the
//! used ring, not blocking hardware polls. The virtio interrupt is only
//! there to *wake* whatever task owns the `Interface` + `VirtioNet` pair so
//! it goes and calls `iface.poll(...)` again; the interrupt handler itself
//! does no ring processing. See the example at the bottom.

#![allow(dead_code)]

extern crate alloc;

use alloc::vec::Vec;
use core::alloc::Layout;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{fence, Ordering};

use smoltcp::phy::{self, Device, DeviceCapabilities, Medium};
use smoltcp::time::Instant;

// ---------------------------------------------------------------------
// virtio-mmio register offsets (same layout you already used for blk)
// ---------------------------------------------------------------------
mod reg {
    pub const MAGIC_VALUE: usize = 0x000;
    pub const VERSION: usize = 0x004;
    pub const DEVICE_ID: usize = 0x008;
    pub const VENDOR_ID: usize = 0x00c;
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
    pub const CONFIG_GENERATION: usize = 0x0fc;
    pub const CONFIG: usize = 0x100;
}

const VIRTIO_MAGIC: u32 = 0x74726976; // "virt"

// Matches virtio_blk.rs's layout exactly -- same QEMU virt-machine mmio bank.
const VIRTIO_MMIO_BASE: usize = crate::mm::layout::phys_to_virt(0x0a00_0000);
const VIRTIO_MMIO_STRIDE: usize = 0x200;
const VIRTIO_MMIO_SLOTS: usize = 32;

/// Scan the same virtio-mmio bank virtio_blk.rs scans, looking for a
/// network device (DEVICE_ID == 1) in modern (version 2) mode.
pub fn find_net_device() -> Option<usize> {
    for slot in 0..VIRTIO_MMIO_SLOTS {
        let base = VIRTIO_MMIO_BASE + slot * VIRTIO_MMIO_STRIDE;
        let magic = unsafe { mmio_read32(base as *mut u8, reg::MAGIC_VALUE) };
        if magic != VIRTIO_MAGIC {
            continue;
        }
        let version = unsafe { mmio_read32(base as *mut u8, reg::VERSION) };
        let device_id = unsafe { mmio_read32(base as *mut u8, reg::DEVICE_ID) };
        if device_id == VIRTIO_NET_DEVICE_ID {
            if version != 2 {
                log::warn!(
                    "virtio-net: slot {} is legacy (version={}), this driver requires version=2. \
                     Add disable-legacy=on to the QEMU -device virtio-net-device line.",
                    slot, version
                );
                continue;
            }
            return Some(base);
        }
    }
    None
}

const STATUS_ACKNOWLEDGE: u32 = 1;
const STATUS_DRIVER: u32 = 2;
const STATUS_DRIVER_OK: u32 = 4;
const STATUS_FEATURES_OK: u32 = 8;

const VIRTIO_NET_DEVICE_ID: u32 = 1;

const VIRTIO_NET_F_MAC: u64 = 1 << 5;
const VIRTIO_NET_F_STATUS: u64 = 1 << 16;
const VIRTIO_F_VERSION_1: u64 = 1 << 32;

const RX_QUEUE: u32 = 0;
const TX_QUEUE: u32 = 1;
const QUEUE_SIZE: u16 = 256; // must be a power of two and <= QueueNumMax

/// virtio-net packet header. With `VIRTIO_F_VERSION_1` negotiated, the
/// `num_buffers` field is always present -> header is always 12 bytes.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioNetHdr {
    flags: u8,
    gso_type: u8,
    hdr_len: u16,
    gso_size: u16,
    csum_start: u16,
    csum_offset: u16,
    num_buffers: u16,
}
const VNET_HDR_LEN: usize = core::mem::size_of::<VirtioNetHdr>(); // 12

/// Real Ethernet MTU we advertise to smoltcp (standard 1500 + 14 byte
/// Ethernet header). Buffers are bigger than this to leave slack.
const ETH_FRAME_LEN: usize = 1514;
/// Per-descriptor DMA buffer size: virtio-net header + Ethernet frame,
/// rounded up.
const FRAME_BUF_LEN: usize = 2048;

// =======================================================================
// Low-level MMIO helpers
// =======================================================================

#[inline]
unsafe fn mmio_read32(base: *mut u8, off: usize) -> u32 {
    read_volatile(base.add(off) as *const u32)
}
#[inline]
unsafe fn mmio_write32(base: *mut u8, off: usize, val: u32) {
    write_volatile(base.add(off) as *mut u32, val)
}

/// Translate a kernel virtual address to the physical address the device
/// DMAs to/from.
#[inline]
fn phys_addr<T>(p: *const T) -> u64 {
    crate::mm::layout::virt_to_phys(p as usize) as u64
}

/// A page-aligned, zeroed DMA-able allocation. Owns its memory; frees on drop.
struct Dma {
    ptr: *mut u8,
    layout: Layout,
}

impl Dma {
    fn alloc_zeroed(size: usize, align: usize) -> Self {
        let layout = Layout::from_size_align(size, align).expect("bad DMA layout");
        let ptr = unsafe { alloc::alloc::alloc_zeroed(layout) };
        assert!(!ptr.is_null(), "virtio-net: DMA allocation failed ({size} bytes)");
        Dma { ptr, layout }
    }

    #[inline]
    fn as_ptr(&self) -> *mut u8 {
        self.ptr
    }

    #[inline]
    fn phys_addr(&self) -> u64 {
        phys_addr(self.ptr)
    }
}

impl Drop for Dma {
    fn drop(&mut self) {
        unsafe { alloc::alloc::dealloc(self.ptr, self.layout) }
    }
}

// =======================================================================
// Split virtqueue (desc / avail / used as three separately-addressed
// regions, as modern virtio-mmio allows -- no legacy contiguous-blob
// alignment math needed)
// =======================================================================

#[repr(C)]
#[derive(Clone, Copy)]
struct Desc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}
const DESC_F_WRITE: u16 = 2;

struct UsedElem {
    id: u32,
    len: u32,
}

struct Virtqueue {
    queue_index: u32,
    size: u16,
    desc: Dma,
    avail: Dma,
    used: Dma,
    free_head: u16,
    num_free: u16,
    last_used_idx: u16,
}

impl Virtqueue {
    /// # Safety
    /// `mmio` must be a valid, mapped virtio-mmio base address for a device
    /// that has already gone through ACKNOWLEDGE|DRIVER|FEATURES_OK, and
    /// this must be called before DRIVER_OK is set.
    unsafe fn new(mmio: *mut u8, queue_index: u32, size: u16) -> Self {
        mmio_write32(mmio, reg::QUEUE_SEL, queue_index);
        let max = mmio_read32(mmio, reg::QUEUE_NUM_MAX);
        assert!(
            max != 0 && (max as u16) >= size,
            "virtio-net: queue {queue_index} max size {max} < requested {size}"
        );
        mmio_write32(mmio, reg::QUEUE_NUM, size as u32);

        let desc_size = size as usize * core::mem::size_of::<Desc>();
        let avail_size = 4 + 2 * size as usize + 2; // flags,idx,ring[],used_event
        let used_size = 4 + 8 * size as usize + 2; // flags,idx,ring[]{id,len},avail_event

        let desc = Dma::alloc_zeroed(desc_size, 16);
        let avail = Dma::alloc_zeroed(avail_size, 2);
        let used = Dma::alloc_zeroed(used_size, 4);

        // Free-list: 0 -> 1 -> 2 -> ... -> size-1 -> (wraps to 0, unused
        // while num_free < size).
        let desc_table = desc.as_ptr() as *mut Desc;
        for i in 0..size {
            let d = Desc { addr: 0, len: 0, flags: 0, next: if i + 1 < size { i + 1 } else { 0 } };
            write_volatile(desc_table.add(i as usize), d);
        }

        let desc_phys = desc.phys_addr();
        let avail_phys = avail.phys_addr();
        let used_phys = used.phys_addr();
        mmio_write32(mmio, reg::QUEUE_DESC_LOW, desc_phys as u32);
        mmio_write32(mmio, reg::QUEUE_DESC_HIGH, (desc_phys >> 32) as u32);
        mmio_write32(mmio, reg::QUEUE_DRIVER_LOW, avail_phys as u32);
        mmio_write32(mmio, reg::QUEUE_DRIVER_HIGH, (avail_phys >> 32) as u32);
        mmio_write32(mmio, reg::QUEUE_DEVICE_LOW, used_phys as u32);
        mmio_write32(mmio, reg::QUEUE_DEVICE_HIGH, (used_phys >> 32) as u32);

        mmio_write32(mmio, reg::QUEUE_READY, 1);

        Virtqueue {
            queue_index,
            size,
            desc,
            avail,
            used,
            free_head: 0,
            num_free: size,
            last_used_idx: 0,
        }
    }

    #[inline]
    fn desc_table(&self) -> *mut Desc {
        self.desc.as_ptr() as *mut Desc
    }

    unsafe fn alloc_desc(&mut self) -> Option<u16> {
        if self.num_free == 0 {
            return None;
        }
        let idx = self.free_head;
        let d = read_volatile(self.desc_table().add(idx as usize));
        self.free_head = d.next;
        self.num_free -= 1;
        Some(idx)
    }

    unsafe fn free_desc(&mut self, idx: u16) {
        let mut d = read_volatile(self.desc_table().add(idx as usize));
        d.next = self.free_head;
        write_volatile(self.desc_table().add(idx as usize), d);
        self.free_head = idx;
        self.num_free += 1;
    }

    #[inline]
    unsafe fn avail_idx(&self) -> u16 {
        read_volatile((self.avail.as_ptr() as *const u16).add(1))
    }
    #[inline]
    unsafe fn avail_set_idx(&self, idx: u16) {
        write_volatile((self.avail.as_ptr() as *mut u16).add(1), idx);
    }
    #[inline]
    unsafe fn avail_ring_set(&self, slot: u16, desc_idx: u16) {
        let ring_ptr = (self.avail.as_ptr() as *mut u16).add(2); // past flags+idx
        write_volatile(ring_ptr.add(slot as usize), desc_idx);
    }

    #[inline]
    unsafe fn used_idx(&self) -> u16 {
        read_volatile((self.used.as_ptr() as *const u16).add(1))
    }
    #[inline]
    unsafe fn used_elem(&self, slot: u16) -> UsedElem {
        let ring_ptr = (self.used.as_ptr() as *const u32).add(1); // past flags+idx (2 x u16)
        let base = ring_ptr.add(slot as usize * 2);
        UsedElem { id: read_volatile(base), len: read_volatile(base.add(1)) }
    }

    /// Point descriptor `idx` at `phys`/`len`, publish it in the avail ring,
    /// and notify the device.
    unsafe fn submit(&mut self, mmio: *mut u8, idx: u16, phys: u64, len: u32, device_writes: bool) {
        let d = Desc {
            addr: phys,
            len,
            flags: if device_writes { DESC_F_WRITE } else { 0 },
            next: 0,
        };
        write_volatile(self.desc_table().add(idx as usize), d);

        let avail_idx = self.avail_idx();
        let slot = avail_idx % self.size;
        self.avail_ring_set(slot, idx);

        fence(Ordering::SeqCst); // descriptor + ring slot visible before idx bump
        self.avail_set_idx(avail_idx.wrapping_add(1));
        fence(Ordering::SeqCst); // idx visible before notify

        mmio_write32(mmio, reg::QUEUE_NOTIFY, self.queue_index);
    }

    /// Pop one completed descriptor from the used ring, if any.
    unsafe fn pop_used(&mut self) -> Option<(u16, u32)> {
        if self.last_used_idx == self.used_idx() {
            return None;
        }
        fence(Ordering::SeqCst);
        let slot = self.last_used_idx % self.size;
        let elem = self.used_elem(slot);
        self.last_used_idx = self.last_used_idx.wrapping_add(1);
        Some((elem.id as u16, elem.len))
    }
}

// =======================================================================
// VirtioNet device
// =======================================================================

pub struct VirtioNet {
    mmio: *mut u8,
    mac: [u8; 6],
    rxq: Virtqueue,
    txq: Virtqueue,
    // Indexed by descriptor index (1:1, since we never chain descriptors).
    rx_bufs: Vec<Dma>,
    tx_bufs: Vec<Dma>,
}

impl VirtioNet {
    /// # Safety
    /// `mmio_base` must point at a valid, mapped virtio-mmio region for a
    /// network device (`DeviceID == 1`) in modern (non-legacy) mode
    /// (`Version == 2`), not yet initialized by any other driver.
    pub unsafe fn new(mmio_base: *mut u8) -> Self {
        let magic = mmio_read32(mmio_base, reg::MAGIC_VALUE);
        assert_eq!(magic, VIRTIO_MAGIC, "virtio-net: bad magic at {mmio_base:p}");
        let version = mmio_read32(mmio_base, reg::VERSION);
        assert_eq!(version, 2, "virtio-net: expected modern transport (version 2), got {version}");
        let device_id = mmio_read32(mmio_base, reg::DEVICE_ID);
        assert_eq!(device_id, VIRTIO_NET_DEVICE_ID, "virtio-net: wrong device id {device_id}");

        // --- virtio device init sequence (virtio 1.x spec 3.1.1) ---
        mmio_write32(mmio_base, reg::STATUS, 0); // reset
        mmio_write32(mmio_base, reg::STATUS, STATUS_ACKNOWLEDGE);
        mmio_write32(mmio_base, reg::STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER);

        // Feature negotiation: minimal set (MAC + STATUS), plus the
        // mandatory VIRTIO_F_VERSION_1 for modern operation.
        mmio_write32(mmio_base, reg::DEVICE_FEATURES_SEL, 0);
        let feat_lo = mmio_read32(mmio_base, reg::DEVICE_FEATURES) as u64;
        mmio_write32(mmio_base, reg::DEVICE_FEATURES_SEL, 1);
        let feat_hi = mmio_read32(mmio_base, reg::DEVICE_FEATURES) as u64;
        let device_features = feat_lo | (feat_hi << 32);

        assert!(
            device_features & VIRTIO_F_VERSION_1 != 0,
            "virtio-net: device doesn't offer VIRTIO_F_VERSION_1"
        );
        let wanted = VIRTIO_NET_F_MAC | VIRTIO_NET_F_STATUS | VIRTIO_F_VERSION_1;
        let negotiated = device_features & wanted;
        assert!(negotiated & VIRTIO_NET_F_MAC != 0, "virtio-net: device doesn't offer MAC feature");

        mmio_write32(mmio_base, reg::DRIVER_FEATURES_SEL, 0);
        mmio_write32(mmio_base, reg::DRIVER_FEATURES, negotiated as u32);
        mmio_write32(mmio_base, reg::DRIVER_FEATURES_SEL, 1);
        mmio_write32(mmio_base, reg::DRIVER_FEATURES, (negotiated >> 32) as u32);

        mmio_write32(
            mmio_base,
            reg::STATUS,
            STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK,
        );
        let status = mmio_read32(mmio_base, reg::STATUS);
        assert!(status & STATUS_FEATURES_OK != 0, "virtio-net: device rejected our feature subset");

        // Queues must be set up before DRIVER_OK.
        let mut rxq = Virtqueue::new(mmio_base, RX_QUEUE, QUEUE_SIZE);
        let txq = Virtqueue::new(mmio_base, TX_QUEUE, QUEUE_SIZE);

        // MAC is valid to read since VIRTIO_NET_F_MAC was negotiated.
        let mut mac = [0u8; 6];
        for (i, byte) in mac.iter_mut().enumerate() {
            *byte = read_volatile(mmio_base.add(reg::CONFIG + i));
        }

        // Allocate RX/TX buffers now, but don't post/notify yet -- do that
        // *after* DRIVER_OK below. QEMU's virtio-net backend doesn't appear
        // to act on QUEUE_NOTIFY doorbells rung before the device is live,
        // so ringing them here would silently go nowhere.
        let mut rx_bufs = Vec::with_capacity(QUEUE_SIZE as usize);
        for _ in 0..QUEUE_SIZE {
            rx_bufs.push(Dma::alloc_zeroed(FRAME_BUF_LEN, 16));
        }
        let mut tx_bufs = Vec::with_capacity(QUEUE_SIZE as usize);
        for _ in 0..QUEUE_SIZE {
            tx_bufs.push(Dma::alloc_zeroed(FRAME_BUF_LEN, 16));
        }

        mmio_write32(
            mmio_base,
            reg::STATUS,
            STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK,
        );

        // Now that the device is live, post every RX descriptor so it has
        // somewhere to write incoming frames.
        for (i, buf) in rx_bufs.iter().enumerate() {
            let phys = buf.phys_addr();
            let desc_idx = rxq.alloc_desc().expect("fresh rx queue must have free descriptors");
            debug_assert_eq!(desc_idx as usize, i, "rx descriptors must be posted in free-list order");
            rxq.submit(mmio_base, desc_idx, phys, FRAME_BUF_LEN as u32, true);
        }

        VirtioNet { mmio: mmio_base, mac, rxq, txq, rx_bufs, tx_bufs }
    }

    /// Convenience constructor matching `BlkDevice::init()`'s shape: scans
    /// for the device, returns `None` (with a logged reason) instead of
    /// panicking if it's missing or rejects our feature set.
    pub fn init() -> Option<Self> {
        let base = find_net_device()?;
        // `VirtioNet::new` does its own asserts on magic/version/device_id,
        // which is fine here since find_net_device() already checked them --
        // this is just the same "safe wrapper around an unsafe ctor" shape
        // BlkDevice::init() uses.
        Some(unsafe { VirtioNet::new(base as *mut u8) })
    }

    pub fn mac_address(&self) -> [u8; 6] {
        self.mac
    }

    /// The MMIO base, for the interrupt handler.
    pub fn mmio_base(&self) -> usize {
        self.mmio as usize
    }

    /// Diagnostic only: read the device's own avail/used indices directly
    /// for both queues, bypassing our own `last_used_idx` bookkeeping.
    /// (rx_avail_idx, rx_used_idx, tx_avail_idx, tx_used_idx)
    pub unsafe fn debug_ring_state(&self) -> (u16, u16, u16, u16) {
        (
            self.rxq.avail_idx(),
            self.rxq.used_idx(),
            self.txq.avail_idx(),
            self.txq.used_idx(),
        )
    }

    /// VIRTIO_NET_S_LINK_UP, valid since VIRTIO_NET_F_STATUS was negotiated.
    pub unsafe fn link_up(&self) -> bool {
        let status = read_volatile(self.mmio.add(reg::CONFIG + 6) as *const u16);
        status & 0x1 != 0
    }

    /// Call from your IRQ dispatch for this device's line. Only
    /// acknowledges at the device level; wake your net task from the
    /// caller so it re-polls the `Interface`. Returns the interrupt status
    /// bits that were acked (bit 0 = used-ring update, bit 1 = config
    /// change) in case you want to log/branch on it.
    pub unsafe fn ack_interrupt(&mut self) -> u32 {
        let status = mmio_read32(self.mmio, reg::INTERRUPT_STATUS);
        mmio_write32(self.mmio, reg::INTERRUPT_ACK, status);
        status
    }

    /// Return completed TX descriptors to the free list. Called
    /// automatically from `transmit()`.
    fn reclaim_tx(&mut self) {
        while let Some((desc_idx, _len)) = unsafe { self.txq.pop_used() } {
            unsafe { self.txq.free_desc(desc_idx) };
        }
    }
}

// =======================================================================
// smoltcp::phy::Device impl
// =======================================================================

pub struct RxToken<'a> {
    mmio: *mut u8,
    rxq: &'a mut Virtqueue,
    desc_idx: u16,
    buf: &'a mut [u8],
    len: usize,
}

impl<'a> phy::RxToken for RxToken<'a> {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        let frame_len = self.len.saturating_sub(VNET_HDR_LEN).min(FRAME_BUF_LEN - VNET_HDR_LEN);
        crate::stats::inc(&crate::stats::NET_RX_PACKETS);
        crate::stats::add(&crate::stats::NET_RX_BYTES, frame_len as u64);
        log::trace!("rx {} bytes (desc {})", frame_len, self.desc_idx);
        f(&self.buf[VNET_HDR_LEN..VNET_HDR_LEN + frame_len])
        // Drop hands the buffer back to the device.
    }
}

impl Drop for RxToken<'_> {
    /// Recycles the buffer whether or not smoltcp consumed the frame, so a
    /// dropped token never leaks a receive descriptor.
    fn drop(&mut self) {
        let phys = phys_addr(self.buf.as_ptr());
        unsafe {
            self.rxq.submit(self.mmio, self.desc_idx, phys, FRAME_BUF_LEN as u32, true);
        }
    }
}

pub struct TxToken<'a> {
    mmio: *mut u8,
    txq: &'a mut Virtqueue,
    tx_bufs: &'a mut [Dma],
}

impl<'a> phy::TxToken for TxToken<'a> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let desc_idx = unsafe { self.txq.alloc_desc() };
        let (Some(desc_idx), true) = (desc_idx, VNET_HDR_LEN + len <= FRAME_BUF_LEN) else {
            // No free descriptor (possible for the token `receive` hands out
            // alongside a frame) or an oversized frame: build it and drop it.
            // TCP will retransmit.
            log::debug!("tx: dropping a {} byte frame (no free descriptor or too big)", len);
            let mut scratch = alloc::vec![0u8; len];
            return f(&mut scratch);
        };
        let buf = &mut self.tx_bufs[desc_idx as usize];
        let slice = unsafe { core::slice::from_raw_parts_mut(buf.as_ptr(), FRAME_BUF_LEN) };

        slice[..VNET_HDR_LEN].fill(0); // zeroed virtio-net header: no offload, 1 buffer
        let result = f(&mut slice[VNET_HDR_LEN..VNET_HDR_LEN + len]);
        crate::stats::inc(&crate::stats::NET_TX_PACKETS);
        crate::stats::add(&crate::stats::NET_TX_BYTES, len as u64);
        log::trace!("tx {} bytes (desc {})", len, desc_idx);

        let phys = phys_addr(buf.as_ptr());
        unsafe {
            self.txq.submit(self.mmio, desc_idx, phys, (VNET_HDR_LEN + len) as u32, false);
        }
        result
    }
}

impl Device for VirtioNet {
    type RxToken<'a> = RxToken<'a> where Self: 'a;
    type TxToken<'a> = TxToken<'a> where Self: 'a;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        self.reclaim_tx();
        let (desc_idx, len) = unsafe { self.rxq.pop_used() }?;
        let buf = self.rx_bufs[desc_idx as usize].as_ptr();
        let buf_slice = unsafe { core::slice::from_raw_parts_mut(buf, FRAME_BUF_LEN) };

        let rx_token = RxToken {
            mmio: self.mmio,
            rxq: &mut self.rxq,
            desc_idx,
            buf: buf_slice,
            len: len as usize,
        };
        let tx_token = TxToken { mmio: self.mmio, txq: &mut self.txq, tx_bufs: &mut self.tx_bufs };
        Some((rx_token, tx_token))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        self.reclaim_tx();
        if self.txq.num_free == 0 {
            return None;
        }
        Some(TxToken { mmio: self.mmio, txq: &mut self.txq, tx_bufs: &mut self.tx_bufs })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = ETH_FRAME_LEN;
        caps.max_burst_size = Some(QUEUE_SIZE as usize);
        caps.medium = Medium::Ethernet;
        caps
    }
}

// =======================================================================
// Notes / things to revisit
// =======================================================================
//
// - `phys_addr()` translates through `mm::layout::virt_to_phys`, so
//   buffers must live in the kernel's linear map (heap, statics, stacks).
//
// - No VIRTIO_NET_F_CTRL_VQ negotiated: no control virtqueue, so no
//   MAC/multicast filter programming. QEMU's virtio-net backend doesn't
//   require one to pass traffic; you'll just receive whatever the host
//   side (user-mode net / tap) delivers.
//
// - Every RX/TX completion raises an interrupt (VIRTIO_RING_F_EVENT_IDX not
//   negotiated). Fine functionally; if interrupt rate becomes a problem
//   under load, negotiating EVENT_IDX and using `used_event`/`avail_event`
//   is the next step.
//
// - `QUEUE_SIZE` is 256 for both queues; drop it if you want a smaller
//   footprint (e.g. 64) -- must stay a power of two.
//
// - Not `Sync`/`Send`-audited: a single owner (crate::net's task) drives
//   `Interface::poll`, which does all the ring work; the interrupt handler
//   only acks and wakes it. Don't call `Device` methods from two cores.
//
