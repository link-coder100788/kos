//! QEMU fw_cfg driver (MMIO variant, aarch64 `virt` machine).
//! Fixed base address per QEMU's virt machine memory map.

use core::ptr::addr_of;

const FW_CFG_DMA_ADDR: usize = phys_to_virt(0x0902_0010);
use crate::mm::layout::{phys_to_virt, virt_to_phys};
pub const FW_CFG_FILE_DIR: u16 = 0x19;

pub const FW_CFG_DMA_CTL_ERROR: u32 = 0x01;
pub const FW_CFG_DMA_CTL_READ: u32 = 0x02;
pub const FW_CFG_DMA_CTL_SELECT: u32 = 0x08;
pub const FW_CFG_DMA_CTL_WRITE: u32 = 0x10;

#[repr(C, packed)]
struct FwCfgDmaAccess {
    control: u32,
    length: u32,
    address: u64,
}

#[repr(C)]
struct FwCfgFile {
    size: u32,
    select: u16,
    reserved: u16,
    name: [u8; 56],
}

/// Issue one fw_cfg DMA transaction and block until QEMU finishes it.
/// `control` must include SELECT (with a selector in bits 16..32) if this
/// is the first access to an item; omit it to continue reading/writing
/// at the current offset of the already-selected item.
fn dma_transfer(control: u32, length: u32, address: u64) -> Result<(), ()> {
    let mut desc = FwCfgDmaAccess {
        control: control.to_be(),
        length: length.to_be(),
        address: address.to_be(),
    };

    // QEMU reads the descriptor itself via DMA, so it needs its physical address.
    let desc_phys_addr = virt_to_phys(&desc as *const FwCfgDmaAccess as usize) as u64;

    unsafe {
        core::ptr::write_volatile(FW_CFG_DMA_ADDR as *mut u64, desc_phys_addr.to_be());
    }

    // QEMU updates `control` in place as the transfer proceeds/completes.
    loop {
        let ctrl = u32::from_be(unsafe { core::ptr::read_volatile(addr_of!(desc.control)) });
        if ctrl & FW_CFG_DMA_CTL_ERROR != 0 {
            return Err(());
        }
        if ctrl == 0 {
            return Ok(());
        }
        core::hint::spin_loop();
    }
}

/// Look up a fw_cfg file by name (e.g. "etc/ramfb") and return its selector key.
pub fn find_file(name: &str) -> Option<u16> {
    find_file_with_size(name).map(|(sel, _)| sel)
}

/// Looks up a fw_cfg file by name; returns its selector and size.
fn find_file_with_size(name: &str) -> Option<(u16, usize)> {
    let mut count: u32 = 0;
    dma_transfer(
        ((FW_CFG_FILE_DIR as u32) << 16) | FW_CFG_DMA_CTL_SELECT | FW_CFG_DMA_CTL_READ,
        4,
        virt_to_phys(&mut count as *mut u32 as usize) as u64,
    )
    .ok()?;
    let count = u32::from_be(count);

    let mut file = FwCfgFile { size: 0, select: 0, reserved: 0, name: [0; 56] };

    for _ in 0..count {
        // No SELECT here — continues reading from wherever the directory
        // read left off, at the same still-selected item (0x19).
        dma_transfer(
            FW_CFG_DMA_CTL_READ,
            core::mem::size_of::<FwCfgFile>() as u32,
            virt_to_phys(&mut file as *mut FwCfgFile as usize) as u64,
        )
        .ok()?;

        let entry_name = core::str::from_utf8(&file.name).unwrap_or("").trim_end_matches('\0');
        if entry_name == name {
            return Some((u16::from_be(file.select), u32::from_be(file.size) as usize));
        }
    }
    None
}

/// Reads fw_cfg file `name` (e.g. one passed with QEMU's
/// `-fw_cfg name=opt/...,string=...`) into `buf`; returns the bytes read.
pub fn read_file(name: &str, buf: &mut [u8]) -> Option<usize> {
    let (selector, size) = find_file_with_size(name)?;
    let len = size.min(buf.len());
    dma_transfer(
        ((selector as u32) << 16) | FW_CFG_DMA_CTL_SELECT | FW_CFG_DMA_CTL_READ,
        len as u32,
        virt_to_phys(buf.as_mut_ptr() as usize) as u64,
    )
    .ok()?;
    Some(len)
}

/// Write `length` bytes from physical address `data_addr` into fw_cfg item `selector`, starting at offset 0.
pub fn write_file(selector: u16, data_addr: u64, length: u32) -> Result<(), ()> {
    dma_transfer(
        ((selector as u32) << 16) | FW_CFG_DMA_CTL_SELECT | FW_CFG_DMA_CTL_WRITE,
        length,
        data_addr,
    )
}