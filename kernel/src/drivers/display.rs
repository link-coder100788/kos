//! The screen: one static XRGB8888 framebuffer, scanned out by whichever
//! display device QEMU has. virtio-gpu wins if both are present; ramfb is
//! the fallback. Draw into `pixels()`, then `flush` what changed (ramfb
//! scans the buffer by itself, virtio-gpu has to be told).

use core::sync::atomic::{AtomicU8, Ordering};
use super::ramfb;
use super::virtio::gpu;
use crate::mm::layout::virt_to_phys;

pub const WIDTH: usize = 1024;
pub const HEIGHT: usize = 768;
const BPP: usize = 4;
pub const STRIDE: usize = WIDTH * BPP;
const SIZE: usize = STRIDE * HEIGHT;

// Static, page-aligned backing store: a fixed, physically contiguous
// address both devices can scan out of.
#[repr(align(4096))]
struct FrameBufferMem([u8; SIZE]);
static mut FRAMEBUFFER: FrameBufferMem = FrameBufferMem([0; SIZE]);

const NONE: u8 = 0;
const RAMFB: u8 = 1;
const VIRTIO_GPU: u8 = 2;
static BACKEND: AtomicU8 = AtomicU8::new(NONE);

/// Finds a display device and points it at the framebuffer. Returns the
/// device's name.
pub fn init() -> Result<&'static str, &'static str> {
    let phys = virt_to_phys(core::ptr::addr_of!(FRAMEBUFFER) as usize);
    let (backend, name) = match gpu::init(phys, WIDTH as u32, HEIGHT as u32) {
        Ok(()) => (VIRTIO_GPU, "virtio-gpu"),
        Err(gpu_err) => {
            ramfb::init(phys, WIDTH as u32, HEIGHT as u32, STRIDE as u32).map_err(|ramfb_err| {
                log::warn!("virtio-gpu: {}", gpu_err);
                ramfb_err
            })?;
            (RAMFB, "ramfb")
        }
    };
    BACKEND.store(backend, Ordering::Release);
    Ok(name)
}

/// The whole framebuffer as pixels, row by row. Only the UI task (and the
/// panic screen, when nothing else runs) draws to it.
pub fn pixels() -> &'static mut [u32] {
    unsafe { core::slice::from_raw_parts_mut(core::ptr::addr_of_mut!(FRAMEBUFFER) as *mut u32, WIDTH * HEIGHT) }
}

/// Puts the given part of the framebuffer on screen.
pub fn flush(x: usize, y: usize, w: usize, h: usize) {
    if BACKEND.load(Ordering::Acquire) == VIRTIO_GPU && w > 0 && h > 0 {
        gpu::flush(x as u32, y as u32, w as u32, h as u32, STRIDE as u32);
    }
}

/// Puts the whole framebuffer on screen from a fault handler: gives up
/// instead of waiting if the display is busy.
pub fn flush_for_panic() {
    if BACKEND.load(Ordering::Acquire) == VIRTIO_GPU {
        gpu::try_flush(0, 0, WIDTH as u32, HEIGHT as u32, STRIDE as u32);
    }
}

pub fn put_pixel(x: usize, y: usize, color: u32) {
    if x >= WIDTH || y >= HEIGHT {
        return;
    }
    unsafe {
        let px = (core::ptr::addr_of_mut!(FRAMEBUFFER) as *mut u32).add(y * WIDTH + x);
        core::ptr::write_volatile(px, color);
    }
}

pub fn clear(color: u32) {
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            put_pixel(x, y, color);
        }
    }
}
