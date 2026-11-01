//! ramfb display: configures QEMU's ramfb device via fw_cfg to scan out a
//! framebuffer (see `drivers::display`). QEMU watches the memory itself, so
//! there's nothing to do after drawing.

use super::fw_cfg;
use crate::mm::layout::virt_to_phys;

// fourcc_code('X','R','2','4') == DRM_FORMAT_XRGB8888: memory bytes are
// B,G,R,X low->high, i.e. writing 0x00RRGGBB as a u32 gives you the color
// you'd expect from an ordinary 0xRRGGBB hex value.
const DRM_FORMAT_XRGB8888: u32 =
    (b'X' as u32) | ((b'R' as u32) << 8) | ((b'2' as u32) << 16) | ((b'4' as u32) << 24);

#[repr(C, packed)]
struct RamfbCfg {
    addr: u64,
    fourcc: u32,
    flags: u32,
    width: u32,
    height: u32,
    stride: u32,
}

/// Points ramfb at the XRGB8888 framebuffer at physical address `fb_phys`.
pub fn init(fb_phys: usize, width: u32, height: u32, stride: u32) -> Result<(), &'static str> {
    let selector = fw_cfg::find_file("etc/ramfb")
        .ok_or("no display: pass -device virtio-gpu-device or -device ramfb to QEMU")?;

    let cfg = RamfbCfg {
        addr: (fb_phys as u64).to_be(),
        fourcc: DRM_FORMAT_XRGB8888.to_be(),
        flags: 0u32.to_be(),
        width: width.to_be(),
        height: height.to_be(),
        stride: stride.to_be(),
    };

    fw_cfg::write_file(
        selector,
        virt_to_phys(&cfg as *const RamfbCfg as usize) as u64,
        core::mem::size_of::<RamfbCfg>() as u32,
    )
    .map_err(|_| "fw_cfg DMA write of ramfb config failed")?;

    Ok(())
}
