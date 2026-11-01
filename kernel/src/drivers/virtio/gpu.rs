//! virtio-gpu (device id 16), 2D only: QEMU's `-device virtio-gpu-device`.
//!
//! The host keeps its own copy of the screen (a "resource") backed by our
//! framebuffer. Drawing doesn't show up by itself: `flush` copies the
//! changed rectangle into the resource and asks the host to redisplay it.
//!
//! Commands go one at a time on the control queue, as a two-descriptor
//! chain (request, then the device's response), and we spin until each
//! completes. That keeps it usable from the panic screen, with interrupts
//! off; a command takes the host well under a millisecond.

use core::ptr::{read_volatile, write_volatile};
use crate::arch::timer;
use crate::drivers::virtio::mmio::{find_devices, Device, VirtQueue, DESC_F_NEXT, DESC_F_WRITE};
use crate::mm::frame;
use crate::mm::layout::phys_to_virt;
use crate::std::lock::spinlock::Spinlock;

const DEVICE_ID: u32 = 16;
const CONTROLQ: u32 = 0;
const QUEUE_SIZE: u16 = 2;
const TIMEOUT_MS: u64 = 1000;

// Command and response types (virtio spec 5.7.6.7).
const CMD_GET_DISPLAY_INFO: u32 = 0x0100;
const CMD_RESOURCE_CREATE_2D: u32 = 0x0101;
const CMD_SET_SCANOUT: u32 = 0x0103;
const CMD_RESOURCE_FLUSH: u32 = 0x0104;
const CMD_TRANSFER_TO_HOST_2D: u32 = 0x0105;
const CMD_RESOURCE_ATTACH_BACKING: u32 = 0x0106;
const RESP_OK_NODATA: u32 = 0x1100;
const RESP_OK_DISPLAY_INFO: u32 = 0x1101;

/// Memory bytes B,G,R,X: a u32 of 0x00RRGGBB, same as ramfb's XRGB8888.
const FORMAT_B8G8R8X8_UNORM: u32 = 2;
const RESOURCE_ID: u32 = 1;
const SCANOUT: u32 = 0;
const MAX_SCANOUTS: usize = 16;
/// Extra pixels redisplayed around each flushed rectangle (see `Gpu::flush`).
const REDISPLAY_MARGIN: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CtrlHdr {
    kind: u32,
    flags: u32,
    fence_id: u64,
    ctx_id: u32,
    ring_idx: u8,
    padding: [u8; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct GpuRect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct DisplayOne {
    r: GpuRect,
    enabled: u32,
    flags: u32,
}

#[repr(C)]
struct RespDisplayInfo {
    hdr: CtrlHdr,
    pmodes: [DisplayOne; MAX_SCANOUTS],
}

#[repr(C)]
struct ResourceCreate2d {
    hdr: CtrlHdr,
    resource_id: u32,
    format: u32,
    width: u32,
    height: u32,
}

/// Attach-backing with its one memory entry right behind it.
#[repr(C)]
struct ResourceAttachBacking {
    hdr: CtrlHdr,
    resource_id: u32,
    nr_entries: u32,
    addr: u64,
    length: u32,
    padding: u32,
}

#[repr(C)]
struct SetScanout {
    hdr: CtrlHdr,
    r: GpuRect,
    scanout_id: u32,
    resource_id: u32,
}

#[repr(C)]
struct TransferToHost2d {
    hdr: CtrlHdr,
    r: GpuRect,
    offset: u64,
    resource_id: u32,
    padding: u32,
}

#[repr(C)]
struct ResourceFlush {
    hdr: CtrlHdr,
    r: GpuRect,
    resource_id: u32,
    padding: u32,
}

fn hdr(kind: u32) -> CtrlHdr {
    CtrlHdr { kind, ..Default::default() }
}

/// Requests are built at the start of `page`, responses land halfway in.
const RESP_OFF: usize = 2048;

struct Gpu {
    dev: Device,
    queue: VirtQueue,
    page: usize,
    width: u32,
    height: u32,
}

static GPU: Spinlock<Option<Gpu>> = Spinlock::new(None);

impl Gpu {
    /// Sends `req` and waits for the response, returning its type (or None
    /// if the device never answered).
    fn command<T>(&mut self, req: T, resp_len: usize) -> Option<u32> {
        let req_len = core::mem::size_of::<T>();
        debug_assert!(req_len <= RESP_OFF && resp_len <= RESP_OFF);
        let virt = phys_to_virt(self.page);
        unsafe {
            write_volatile(virt as *mut T, req);
            write_volatile((virt + RESP_OFF) as *mut u32, 0);
        }
        self.queue.set_buffer(0, self.page, req_len as u32, DESC_F_NEXT, 1);
        self.queue.set_buffer(1, self.page + RESP_OFF, resp_len as u32, DESC_F_WRITE, 0);
        self.queue.make_available(0);
        self.queue.notify(&self.dev, CONTROLQ);
        let deadline = timer::uptime_ms() + TIMEOUT_MS;
        loop {
            if self.queue.pop_used().is_some() {
                break;
            }
            if timer::uptime_ms() > deadline {
                log::error!("command {:#x} timed out", unsafe { read_volatile(virt as *const u32) });
                return None;
            }
            core::hint::spin_loop();
        }
        // Nobody listens for the interrupt; just keep it from staying raised.
        self.dev.ack_interrupt();
        Some(unsafe { read_volatile((virt + RESP_OFF) as *const u32) })
    }

    /// Like `command`, for commands answered with a plain OK.
    fn command_ok<T>(&mut self, req: T) -> Result<(), &'static str> {
        match self.command(req, core::mem::size_of::<CtrlHdr>()) {
            Some(RESP_OK_NODATA) => Ok(()),
            Some(resp) => {
                log::error!("command failed: response {:#x}", resp);
                Err("virtio-gpu command failed")
            }
            None => Err("virtio-gpu command timed out"),
        }
    }

    /// The host's preferred size for scanout 0, if it has one.
    fn display_info(&mut self) -> Option<(u32, u32)> {
        if self.command(hdr(CMD_GET_DISPLAY_INFO), core::mem::size_of::<RespDisplayInfo>())? != RESP_OK_DISPLAY_INFO {
            return None;
        }
        let info = unsafe { &*((phys_to_virt(self.page) + RESP_OFF) as *const RespDisplayInfo) };
        let mode = info.pmodes[SCANOUT as usize];
        (mode.enabled != 0).then_some((mode.r.width, mode.r.height))
    }

    /// Copies the rectangle to the host, then has it redisplay a slightly
    /// bigger one: QEMU's Cocoa window, when scaled, can round a redisplay
    /// inwards and leave the edge pixels stale (e.g. bits of an old cursor).
    fn flush(&mut self, x: u32, y: u32, width: u32, height: u32, stride: u32) {
        let r = GpuRect { x, y, width, height };
        let (x0, y0) = (x.saturating_sub(REDISPLAY_MARGIN), y.saturating_sub(REDISPLAY_MARGIN));
        let x1 = (x + width + REDISPLAY_MARGIN).min(self.width);
        let y1 = (y + height + REDISPLAY_MARGIN).min(self.height);
        let shown = GpuRect { x: x0, y: y0, width: x1 - x0, height: y1 - y0 };
        let offset = y as u64 * stride as u64 + x as u64 * 4;
        let ok = self.command_ok(TransferToHost2d { hdr: hdr(CMD_TRANSFER_TO_HOST_2D), r, offset, resource_id: RESOURCE_ID, padding: 0 })
            .and_then(|()| self.command_ok(ResourceFlush { hdr: hdr(CMD_RESOURCE_FLUSH), r: shown, resource_id: RESOURCE_ID, padding: 0 }));
        if ok.is_err() {
            log::error!("flush of {}x{} at ({}, {}) failed", width, height, x, y);
        }
    }
}

/// Finds the device and has it scan out the XRGB8888 framebuffer at
/// physical address `fb_phys`.
pub fn init(fb_phys: usize, width: u32, height: u32) -> Result<(), &'static str> {
    let dev = find_devices(DEVICE_ID).next().ok_or("no device")?;
    if !dev.begin_init(0) {
        return Err("device rejected our features");
    }
    let queue = VirtQueue::new(&dev, CONTROLQ, QUEUE_SIZE).ok_or("control queue unavailable")?;
    let page = frame::alloc_zeroed().ok_or("out of memory")?;
    dev.finish_init();
    let mut gpu = Gpu { dev, queue, page, width, height };

    if let Some((w, h)) = gpu.display_info() {
        log::debug!("host prefers {}x{}", w, h);
    }
    gpu.command_ok(ResourceCreate2d {
        hdr: hdr(CMD_RESOURCE_CREATE_2D),
        resource_id: RESOURCE_ID,
        format: FORMAT_B8G8R8X8_UNORM,
        width,
        height,
    })?;
    gpu.command_ok(ResourceAttachBacking {
        hdr: hdr(CMD_RESOURCE_ATTACH_BACKING),
        resource_id: RESOURCE_ID,
        nr_entries: 1,
        addr: fb_phys as u64,
        length: width * height * 4,
        padding: 0,
    })?;
    gpu.command_ok(SetScanout {
        hdr: hdr(CMD_SET_SCANOUT),
        r: GpuRect { x: 0, y: 0, width, height },
        scanout_id: SCANOUT,
        resource_id: RESOURCE_ID,
    })?;
    gpu.flush(0, 0, width, height, width * 4);
    log::info!("virtio-gpu at slot {}", dev.slot());
    *GPU.lock() = Some(gpu);
    Ok(())
}

/// Puts a rectangle of the framebuffer on screen.
pub fn flush(x: u32, y: u32, width: u32, height: u32, stride: u32) {
    if let Some(gpu) = GPU.lock().as_mut() {
        gpu.flush(x, y, width, height, stride);
    }
}

/// `flush`, unless someone is mid-command (e.g. the core that just faulted).
pub fn try_flush(x: u32, y: u32, width: u32, height: u32, stride: u32) {
    if let Some(mut guard) = GPU.try_lock() {
        if let Some(gpu) = guard.as_mut() {
            gpu.flush(x, y, width, height, stride);
        }
    }
}
