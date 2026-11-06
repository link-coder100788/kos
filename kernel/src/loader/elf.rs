use elf::ElfBytes;
use elf::endian::AnyEndian;
use core::ops::Range;
use super::ProgramMemory;

#[derive(Debug)]
pub enum LoadError {
    Base64,
    Parse,
    NoSegments,
    SegmentData,
    OutOfRange,
    /// A relocation type the loader doesn't handle (only RELATIVE is).
    UnsupportedRelocation(u32),
    /// Couldn't map or write the program's memory.
    OutOfMemory(&'static str),
}

/// Loads an ELF into `mem`. A position-independent one (clang's) runs at
/// `load_base`; a fixed-address one (tcc's, linked at `USER_BASE`) runs
/// where it was linked to. Everything it puts in memory must land inside
/// `region`. Returns the entry point and where the image ends.
pub fn load_elf(elf_bytes: &[u8], load_base: usize, region: Range<usize>, mem: &mut dyn ProgramMemory) -> Result<(usize, usize), LoadError> {
    let file = ElfBytes::<AnyEndian>::minimal_parse(elf_bytes).map_err(|_| LoadError::Parse)?;
    let fixed = file.ehdr.e_type == elf::abi::ET_EXEC;
    let mut image_end = load_base;
    // A fixed-address program's addresses are already the real ones.
    let load_base = if fixed { 0 } else { load_base };
    let segments = file.segments().ok_or(LoadError::NoSegments)?;
    for phdr in segments {
        if phdr.p_type != elf::abi::PT_LOAD {
            continue;
        }
        let target_addr = load_base + phdr.p_vaddr as usize;
        log::debug!("segment at {:#x}: {} bytes from file, {} in memory, flags {:#x}", target_addr, phdr.p_filesz, phdr.p_memsz, phdr.p_flags);
        let seg_end = target_addr
            .checked_add(phdr.p_memsz as usize)
            .ok_or(LoadError::OutOfRange)?;
        if target_addr < region.start || seg_end > region.end {
            return Err(LoadError::OutOfRange);
        }
        image_end = image_end.max(seg_end);
        let data = file.segment_data(&phdr).map_err(|_| LoadError::SegmentData)?;
        if data.len() > phdr.p_memsz as usize {
            return Err(LoadError::SegmentData);
        }
        // Fresh memory is zeroed, which takes care of the .bss part.
        mem.map(target_addr, phdr.p_memsz as usize).map_err(LoadError::OutOfMemory)?;
        mem.write(target_addr, data).map_err(LoadError::OutOfMemory)?;
    }
    if !fixed {
        apply_relocations(&file, load_base, &region, mem)?;
    }
    let entry = load_base
        .checked_add(file.ehdr.e_entry as usize)
        .ok_or(LoadError::OutOfRange)?;
    if !region.contains(&entry) {
        return Err(LoadError::OutOfRange);
    }
    Ok((entry, image_end))
}

/// Fixes up pointers stored in the program's data. A position-independent
/// program can't know where it'll be loaded, so the linker can't fill in a
/// pointer in, say, a table of strings; it records a RELATIVE relocation
/// instead ("this 8-byte slot holds the load address plus this addend"),
/// and the loader fills it in. Without this, such pointers stay 0.
fn apply_relocations(file: &ElfBytes<AnyEndian>, load_base: usize, region: &Range<usize>, mem: &mut dyn ProgramMemory) -> Result<(), LoadError> {
    let Some(section) = file.section_header_by_name(".rela.dyn").map_err(|_| LoadError::Parse)? else {
        return Ok(());
    };
    let relocations = file.section_data_as_relas(&section).map_err(|_| LoadError::Parse)?;
    let mut count = 0;
    for rela in relocations {
        if rela.r_type != elf::abi::R_AARCH64_RELATIVE {
            return Err(LoadError::UnsupportedRelocation(rela.r_type));
        }
        let slot = load_base.checked_add(rela.r_offset as usize).ok_or(LoadError::OutOfRange)?;
        if slot < region.start || slot.checked_add(8).is_none_or(|end| end > region.end) {
            return Err(LoadError::OutOfRange);
        }
        let value = (load_base as i64).wrapping_add(rela.r_addend) as u64;
        mem.write(slot, &value.to_le_bytes()).map_err(LoadError::OutOfMemory)?;
        count += 1;
    }
    log::debug!("{} relocations applied", count);
    Ok(())
}
