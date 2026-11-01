//! kbin_loader.rs — loads a KBIN image (produced by `kc`, the K compiler)
//! into userspace memory and returns its entry point, mirroring
//! `load_pie_elf`'s shape so it drops into the same runner/scheduler flow.
//!
//! KBIN header (see kbin.rs in the K compiler):
//!   Magic         4 bytes   "KBIN"
//!   Version       u32
//!   Entry address u64
//!   Code size     u64
//!   Data size     u64
//!   Code          [u8; code_size]
//!   Data          [u8; data_size]
//!
//! IMPORTANT — unlike ELF, KBIN images are position-DEPENDENT, not PIE.
//! Every absolute address baked into the code (string-literal pointers,
//! etc.) assumes the code section starts at exactly `entry_address`. There
//! is no `load_base` parameter to choose here: the image tells you where
//! it must go, and the loader's job is just to validate that and place it
//! there. The K compiler always sets `entry_address` to a fixed
//! `LOAD_BASE` (0x5000_0000), which is where every program's own address
//! space starts (see `process`), so several K programs can still run at
//! once, each in its own address space.

use core::ops::Range;

pub const KBIN_MAGIC: &[u8; 4] = b"KBIN";
pub const KBIN_HEADER_LEN: usize = 32;
pub const KBIN_VERSION: u32 = 1;

#[derive(Debug)]
pub enum KbinLoadError {
    TooShort,
    BadMagic,
    UnsupportedVersion(u32),
    SizeMismatch,
    OutOfRange,
    OutOfMemory(&'static str),
}

/// Copies a KBIN image's code and data sections into `mem` at the address
/// baked into its header, which must be inside `region`. Returns the entry
/// point and where the image ends.
pub fn load_kbin(kbin_bytes: &[u8], region: Range<usize>, mem: &mut dyn crate::loader::ProgramMemory) -> Result<(usize, usize), KbinLoadError> {
    if kbin_bytes.len() < KBIN_HEADER_LEN {
        return Err(KbinLoadError::TooShort);
    }
    if &kbin_bytes[0..4] != KBIN_MAGIC {
        return Err(KbinLoadError::BadMagic);
    }
    let version = u32::from_le_bytes(kbin_bytes[4..8].try_into().unwrap());
    if version != KBIN_VERSION {
        return Err(KbinLoadError::UnsupportedVersion(version));
    }
    let entry_address = u64::from_le_bytes(kbin_bytes[8..16].try_into().unwrap()) as usize;
    let code_size = u64::from_le_bytes(kbin_bytes[16..24].try_into().unwrap()) as usize;
    let data_size = u64::from_le_bytes(kbin_bytes[24..32].try_into().unwrap()) as usize;
    // Slice the file into code/data, validating the header's declared
    // sizes actually match how much data is present.
    let code_start = KBIN_HEADER_LEN;
    let code_end = code_start.checked_add(code_size).ok_or(KbinLoadError::SizeMismatch)?;
    let data_start = code_end;
    let data_end = data_start.checked_add(data_size).ok_or(KbinLoadError::SizeMismatch)?;
    if kbin_bytes.len() != data_end {
        return Err(KbinLoadError::SizeMismatch);
    }
    // The compiler always emits the entry stub (`bl main; svc #0`) as the
    // very first instruction, so `entry_address` doubles as the code
    // section's base address — there's no separate "load base" to track.
    let code_base = entry_address;
    let code_target_end = code_base.checked_add(code_size).ok_or(KbinLoadError::OutOfRange)?;
    if code_base < region.start || code_target_end > region.end {
        return Err(KbinLoadError::OutOfRange);
    }
    // Data section starts immediately after code, 8-byte aligned. This
    // MUST match aarch64_codegen::generate()'s layout math exactly —
    // `(LOAD_BASE + code_size_bytes + 7) & !7` — since string-literal
    // addresses were baked into the code assuming this exact placement.
    let data_base = (code_base + code_size + 7) & !7;
    if data_size > 0 {
        let data_target_end = data_base.checked_add(data_size).ok_or(KbinLoadError::OutOfRange)?;
        if data_base < region.start || data_target_end > region.end {
            return Err(KbinLoadError::OutOfRange);
        }
    }
    let end = (data_base + data_size).max(code_target_end);
    mem.map(code_base, end - code_base).map_err(KbinLoadError::OutOfMemory)?;
    mem.write(code_base, &kbin_bytes[code_start..code_end]).map_err(KbinLoadError::OutOfMemory)?;
    if data_size > 0 {
        mem.write(data_base, &kbin_bytes[data_start..data_end]).map_err(KbinLoadError::OutOfMemory)?;
    }
    Ok((entry_address, (data_base + data_size).max(code_target_end)))
}
