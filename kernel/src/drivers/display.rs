//! The screen: one XRGB8888 framebuffer in physically contiguous frames,
//! scanned out by whichever display device QEMU has. virtio-gpu wins if
//! both are present; ramfb is the fallback. Draw into `pixels()`, then
//! `flush` what changed (ramfb scans the buffer by itself, virtio-gpu has
//! to be told). `set_mode` changes the resolution while running.

use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use super::ramfb;
use super::virtio::gpu;
use crate::mm::frame;
use crate::mm::layout::{phys_to_virt, PAGE_SIZE};

const BPP: usize = 4;
/// Neither device takes more than this; it keeps a bad setting from
/// asking for all of memory.
const MAX_SIDE: usize = 4096;

// The current mode. Width is 0 until there's a framebuffer, which makes
// drawing a no-op (e.g. a panic before the UI starts).
static WIDTH: AtomicUsize = AtomicUsize::new(0);
static HEIGHT: AtomicUsize = AtomicUsize::new(0);
/// Physical address of the framebuffer.
static FB_PHYS: AtomicUsize = AtomicUsize::new(0);

const NONE: u8 = 0;
const RAMFB: u8 = 1;
const VIRTIO_GPU: u8 = 2;
static BACKEND: AtomicU8 = AtomicU8::new(NONE);

pub fn width() -> usize {
    WIDTH.load(Ordering::Acquire)
}

pub fn height() -> usize {
    HEIGHT.load(Ordering::Acquire)
}

fn stride() -> usize {
    width() * BPP
}

fn pages_for(width: usize, height: usize) -> usize {
    (width * height * BPP).div_ceil(PAGE_SIZE)
}

/// A zeroed framebuffer for `width` x `height`: its physical address.
fn alloc_framebuffer(width: usize, height: usize) -> Result<usize, &'static str> {
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err("unsupported resolution");
    }
    let pages = pages_for(width, height);
    let phys = frame::alloc_contiguous(pages).ok_or("not enough memory for the framebuffer")?;
    unsafe { core::ptr::write_bytes(phys_to_virt(phys) as *mut u8, 0, pages * PAGE_SIZE) };
    Ok(phys)
}

/// Finds a display device and has it show a `width` x `height` screen.
/// Returns the device's name.
pub fn init(width: usize, height: usize) -> Result<&'static str, &'static str> {
    let phys = alloc_framebuffer(width, height)?;
    let (backend, name) = match gpu::init(phys, width as u32, height as u32) {
        Ok(()) => (VIRTIO_GPU, "virtio-gpu"),
        Err(gpu_err) => match ramfb::init(phys, width as u32, height as u32, (width * BPP) as u32) {
            Ok(()) => (RAMFB, "ramfb"),
            Err(ramfb_err) => {
                log::warn!("virtio-gpu: {}", gpu_err);
                free_framebuffer(phys, width, height);
                return Err(ramfb_err);
            }
        },
    };
    FB_PHYS.store(phys, Ordering::Release);
    HEIGHT.store(height, Ordering::Release);
    WIDTH.store(width, Ordering::Release);
    BACKEND.store(backend, Ordering::Release);
    Ok(name)
}

fn free_framebuffer(phys: usize, width: usize, height: usize) {
    for page in 0..pages_for(width, height) {
        frame::free(phys + page * PAGE_SIZE);
    }
}

/// Switches the screen to `width` x `height`, with a new (black)
/// framebuffer. Only the UI task may call this, as it's the one drawing.
pub fn set_mode(width: usize, height: usize) -> Result<(), &'static str> {
    let (old_w, old_h, old_phys) = (self::width(), self::height(), FB_PHYS.load(Ordering::Acquire));
    if (width, height) == (old_w, old_h) {
        return Ok(());
    }
    let phys = alloc_framebuffer(width, height)?;
    let switched = match BACKEND.load(Ordering::Acquire) {
        VIRTIO_GPU => gpu::set_mode(phys, width as u32, height as u32),
        RAMFB => ramfb::init(phys, width as u32, height as u32, (width * BPP) as u32),
        _ => Err("no display"),
    };
    if let Err(e) = switched {
        free_framebuffer(phys, width, height);
        return Err(e);
    }
    // Shrink first, so a panic screen drawn meanwhile stays inside both.
    WIDTH.store(width.min(old_w), Ordering::Release);
    HEIGHT.store(height.min(old_h), Ordering::Release);
    FB_PHYS.store(phys, Ordering::Release);
    HEIGHT.store(height, Ordering::Release);
    WIDTH.store(width, Ordering::Release);
    free_framebuffer(old_phys, old_w, old_h);
    log::info!("display now {}x{}", width, height);
    Ok(())
}

/// The whole framebuffer as pixels, row by row. Only the UI task (and the
/// panic screen, when nothing else runs) draws to it.
pub fn pixels() -> &'static mut [u32] {
    let fb = phys_to_virt(FB_PHYS.load(Ordering::Acquire)) as *mut u32;
    unsafe { core::slice::from_raw_parts_mut(fb, width() * height()) }
}

/// Puts the given part of the framebuffer on screen.
pub fn flush(x: usize, y: usize, w: usize, h: usize) {
    if BACKEND.load(Ordering::Acquire) == VIRTIO_GPU && w > 0 && h > 0 {
        gpu::flush(x as u32, y as u32, w as u32, h as u32, stride() as u32);
    }
}

/// Puts the whole framebuffer on screen from a fault handler: gives up
/// instead of waiting if the display is busy.
pub fn flush_for_panic() {
    if BACKEND.load(Ordering::Acquire) == VIRTIO_GPU {
        gpu::try_flush(0, 0, width() as u32, height() as u32, stride() as u32);
    }
}

pub fn put_pixel(x: usize, y: usize, color: u32) {
    let w = width();
    if x >= w || y >= height() {
        return;
    }
    unsafe {
        let px = (phys_to_virt(FB_PHYS.load(Ordering::Acquire)) as *mut u32).add(y * w + x);
        core::ptr::write_volatile(px, color);
    }
}

pub fn clear(color: u32) {
    for y in 0..height() {
        for x in 0..width() {
            put_pixel(x, y, color);
        }
    }
}
