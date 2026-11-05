//! AArch64 stage-1 translation (EL1&0): page-table format, a mapper, and
//! turning the MMU on.
//!
//! Configuration: 4 KiB granule, 39-bit virtual addresses (T0SZ = 25), so a
//! walk is three levels:
//!
//! ```text
//!   L1: 512 x 1 GiB    L2: 512 x 2 MiB    L3: 512 x 4 KiB
//!   VA bits [38:30]    VA bits [29:21]    VA bits [20:12]
//! ```
//!
//! TTBR1 (addresses 0xFFFF_FF80_...) holds the kernel, TTBR0 the low half.
//! The mapper works the same for both: only VA bits \[38:12\] matter to the
//! walk.
//!
//! Every program has its own TTBR0 table, tagged with an ASID (address
//! space ID) in TTBR0's top bits. Its mappings are marked not-global, so
//! the TLB keeps them apart by ASID and switching programs needs no TLB
//! flush; only reusing an ASID does (`flush_asid`).

use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::mm::frame;
use crate::mm::layout::{PAGE_SIZE, phys_to_virt};

const ENTRIES: usize = 512;
const L2_BLOCK: usize = 2 * 1024 * 1024;

// ---- descriptor bits -------------------------------------------------------

const DESC_VALID: u64 = 1 << 0;
/// At L1/L2: next level is a table (vs. a block). At L3: must be set (page).
const DESC_TABLE: u64 = 1 << 1;
const DESC_PAGE: u64 = 1 << 1;
const fn attr_index(i: u64) -> u64 { i << 2 }
const AP_EL0: u64 = 1 << 6;
const AP_RO: u64 = 1 << 7;
const SH_INNER: u64 = 0b11 << 8;
const AF: u64 = 1 << 10;
/// Not global: the TLB entry belongs to the current ASID only.
const NG: u64 = 1 << 11;
const PXN: u64 = 1 << 53;
const UXN: u64 = 1 << 54;
const ADDR_MASK: u64 = 0x0000_ffff_ffff_f000;

// MAIR_EL1 slots. Descriptors pick one via AttrIndx.
const MAIR_DEVICE: u64 = 0;      // Device-nGnRE
const MAIR_NORMAL: u64 = 1;      // Normal, write-back cacheable
const MAIR_NORMAL_NC: u64 = 2;   // Normal, non-cacheable
const MAIR_VALUE: u64 = (0x04 << (8 * MAIR_DEVICE)) | (0xff << (8 * MAIR_NORMAL)) | (0x44 << (8 * MAIR_NORMAL_NC));

/// What kind of memory a mapping is and who may do what with it.
#[derive(Clone, Copy, Debug)]
pub struct MapFlags {
    pub device: bool,
    pub writable: bool,
    pub executable: bool,
    pub user: bool,
}

impl core::fmt::Display for MapFlags {
    /// e.g. `mem r-x`, `dev rw-`, `usr rwx`.
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        let kind = if self.device { "dev" } else if self.user { "usr" } else { "mem" };
        write!(f, "{} r{}{}", kind, if self.writable { 'w' } else { '-' }, if self.executable { 'x' } else { '-' })
    }
}

impl MapFlags {
    pub const KERNEL_RX: Self = Self { device: false, writable: false, executable: true, user: false };
    pub const KERNEL_RO: Self = Self { device: false, writable: false, executable: false, user: false };
    pub const KERNEL_RW: Self = Self { device: false, writable: true, executable: false, user: false };
    pub const KERNEL_RWX: Self = Self { device: false, writable: true, executable: true, user: false };
    pub const DEVICE: Self = Self { device: true, writable: true, executable: false, user: false };
    pub const USER_RWX: Self = Self { device: false, writable: true, executable: true, user: true };
    pub const USER_RW: Self = Self { device: false, writable: true, executable: false, user: true };

    /// Attribute bits shared by block and page descriptors.
    fn bits(self) -> u64 {
        let mut bits = AF;
        if self.device {
            bits |= attr_index(MAIR_DEVICE);
        } else {
            bits |= attr_index(MAIR_NORMAL) | SH_INNER;
        }
        if !self.writable { bits |= AP_RO; }
        if self.user { bits |= AP_EL0 | NG; }
        // Never let the kernel execute user pages, and never execute device memory.
        if !self.executable || self.user || self.device { bits |= PXN; }
        if !self.executable || !self.user { bits |= UXN; }
        bits
    }
}

// ---- page tables -----------------------------------------------------------

#[repr(C, align(4096))]
struct Table([u64; ENTRIES]);

fn table_at(pa: usize) -> &'static mut Table {
    unsafe { &mut *(phys_to_virt(pa) as *mut Table) }
}

fn index(va: usize, level: usize) -> usize {
    (va >> (12 + 9 * (3 - level))) & (ENTRIES - 1)
}

/// One translation table hierarchy, i.e. one address space.
pub struct AddressSpace {
    root: usize,
}

impl AddressSpace {
    pub fn new() -> Self {
        Self { root: frame::alloc_zeroed().expect("out of frames for page table root") }
    }

    /// Physical address of the L1 table, for TTBR0/TTBR1.
    pub fn root(&self) -> usize {
        self.root
    }

    /// Maps `len` bytes at `va` to `pa`. All three must be page-aligned. Uses
    /// 2 MiB blocks where alignment allows, 4 KiB pages elsewhere. Remapping
    /// an already-mapped range is a bug and panics.
    pub fn map(&mut self, va: usize, pa: usize, len: usize, flags: MapFlags) {
        assert!(va % PAGE_SIZE == 0 && pa % PAGE_SIZE == 0 && len % PAGE_SIZE == 0,
            "unaligned map {:#x} -> {:#x} ({:#x})", va, pa, len);
        let mut off = 0;
        while off < len {
            let (v, p) = (va + off, pa + off);
            if v % L2_BLOCK == 0 && p % L2_BLOCK == 0 && len - off >= L2_BLOCK {
                self.map_block(v, p, flags);
                off += L2_BLOCK;
            } else {
                self.map_page(v, p, flags);
                off += PAGE_SIZE;
            }
        }
    }

    /// Returns the next-level table behind `table[idx]`, allocating it if empty.
    fn next_table(table: &mut Table, idx: usize) -> &'static mut Table {
        let entry = table.0[idx];
        if entry & DESC_VALID == 0 {
            let pa = frame::alloc_zeroed().expect("out of frames for page table");
            table.0[idx] = pa as u64 | DESC_TABLE | DESC_VALID;
            table_at(pa)
        } else {
            assert!(entry & DESC_TABLE != 0, "mapping inside an existing block");
            table_at((entry & ADDR_MASK) as usize)
        }
    }

    fn map_block(&mut self, va: usize, pa: usize, flags: MapFlags) {
        let l1 = table_at(self.root);
        let l2 = Self::next_table(l1, index(va, 1));
        let slot = &mut l2.0[index(va, 2)];
        assert!(*slot & DESC_VALID == 0, "va {:#x} already mapped", va);
        *slot = pa as u64 | flags.bits() | DESC_VALID;
    }

    fn map_page(&mut self, va: usize, pa: usize, flags: MapFlags) {
        let l1 = table_at(self.root);
        let l2 = Self::next_table(l1, index(va, 1));
        let l3 = Self::next_table(l2, index(va, 2));
        let slot = &mut l3.0[index(va, 3)];
        assert!(*slot & DESC_VALID == 0, "va {:#x} already mapped", va);
        *slot = pa as u64 | flags.bits() | DESC_PAGE | DESC_VALID;
    }

    /// Removes the 4 KiB page mapped at `va` and returns its physical
    /// address. The TLB may still hold it: `flush_page` before reusing
    /// the frame. Empty tables stay.
    pub fn unmap_page(&mut self, va: usize) -> Option<usize> {
        let mut table = table_at(self.root);
        for level in 1..3 {
            let entry = table.0[index(va, level)];
            if entry & DESC_VALID == 0 || entry & DESC_TABLE == 0 {
                return None;
            }
            table = table_at((entry & ADDR_MASK) as usize);
        }
        let slot = &mut table.0[index(va, 3)];
        if *slot & DESC_VALID == 0 {
            return None;
        }
        let pa = (*slot & ADDR_MASK) as usize;
        *slot = 0;
        Some(pa)
    }

    /// Frees every table frame in `table` (at `level`) and below. The
    /// memory the tables map isn't touched.
    fn free_tables(table_pa: usize, level: usize) {
        if level < 3 {
            for &entry in table_at(table_pa).0.iter() {
                if entry & DESC_VALID != 0 && entry & DESC_TABLE != 0 {
                    Self::free_tables((entry & ADDR_MASK) as usize, level + 1);
                }
            }
        }
        frame::free(table_pa);
    }

    /// Software walk: the physical address `va` maps to, if any.
    pub fn translate(&self, va: usize) -> Option<usize> {
        let mut table = table_at(self.root);
        for level in 1..=3 {
            let entry = table.0[index(va, level)];
            if entry & DESC_VALID == 0 {
                return None;
            }
            let span = 1usize << (12 + 9 * (3 - level));
            let is_leaf = level == 3 || entry & DESC_TABLE == 0;
            if is_leaf {
                return Some((entry & ADDR_MASK) as usize & !(span - 1) | (va & (span - 1)));
            }
            table = table_at((entry & ADDR_MASK) as usize);
        }
        None
    }
}

impl Drop for AddressSpace {
    /// Frees the page tables. Nothing may still be using them: switch
    /// TTBR0 away and `flush_asid` first.
    fn drop(&mut self) {
        Self::free_tables(self.root, 1);
    }
}

// ---- switching address spaces ---------------------------------------------

/// The TTBR0 value for `space` tagged with `asid` (1..=255; 0 is the
/// kernel's own low half).
pub fn ttbr0_value(space: &AddressSpace, asid: u16) -> u64 {
    space.root() as u64 | (asid as u64) << 48
}

/// The TTBR0 value kernel tasks run with (the boot stubs, ASID 0).
pub fn kernel_ttbr0() -> u64 {
    KERNEL_TTBR0.load(Ordering::Relaxed)
}

/// Loads a new low half on this core. With ASIDs, no TLB flush is needed.
pub fn set_ttbr0(value: u64) {
    unsafe { asm!("msr ttbr0_el1, {0}", "isb", in(reg) value) };
}

/// Drops the TLB entries for the page at `va` in address space `asid`, on
/// every core, after it was unmapped.
pub fn flush_page(asid: u16, va: usize) {
    let arg = (asid as u64) << 48 | (va as u64 >> 12) & 0xfff_ffff_ffff;
    unsafe { asm!("dsb ishst", "tlbi vae1is, {0}", "dsb ish", "isb", in(reg) arg) };
}

/// Drops every TLB entry tagged with `asid`, before the ASID is reused.
pub fn flush_asid(asid: u16) {
    unsafe { asm!("dsb ishst", "tlbi aside1is, {0}", "dsb ish", "isb", in(reg) (asid as u64) << 48) };
}

// ---- turning it on ---------------------------------------------------------

/// Table roots every core loads; set by `enable` on the boot core.
static KERNEL_TTBR0: AtomicU64 = AtomicU64::new(0);
static KERNEL_TTBR1: AtomicU64 = AtomicU64::new(0);

fn tcr_value() -> u64 {
    let t0sz = 25u64;                 // 39-bit VA in the low half
    let irgn0 = 0b01 << 8;            // walks: inner write-back cacheable
    let orgn0 = 0b01 << 10;           // walks: outer write-back cacheable
    let sh0 = 0b11 << 12;             // walks: inner shareable
    let tg0 = 0b00 << 14;             // 4 KiB granule
    let t1sz = 25u64 << 16;           // 39-bit VA in the high half
    let irgn1 = 0b01 << 24;
    let orgn1 = 0b01 << 26;
    let sh1 = 0b11 << 28;
    let tg1 = 0b10 << 30;             // 4 KiB granule (TG1 encoding differs from TG0)
    let parange: u64;
    unsafe { asm!("mrs {0}, id_aa64mmfr0_el1", out(reg) parange) };
    let ips = (parange & 0b111).min(0b101) << 32;
    t0sz | irgn0 | orgn0 | sh0 | tg0 | t1sz | irgn1 | orgn1 | sh1 | tg1 | ips
}

/// Switches this core to `kernel` (TTBR1, the high half) and `user`
/// (TTBR0, the low half). Both must map the code that's running.
pub fn enable(kernel: &AddressSpace, user: &AddressSpace) {
    KERNEL_TTBR1.store(kernel.root() as u64, Ordering::SeqCst);
    KERNEL_TTBR0.store(user.root() as u64, Ordering::SeqCst);
    enable_on_this_core();
}

/// Moves a secondary core from the boot tables (arch/boot.s) onto the
/// boot core's tables. A no-op if the boot core hasn't called `enable` yet.
pub fn enable_on_this_core() {
    let ttbr0 = KERNEL_TTBR0.load(Ordering::SeqCst);
    let ttbr1 = KERNEL_TTBR1.load(Ordering::SeqCst);
    if ttbr0 == 0 {
        return;
    }
    unsafe {
        asm!(
            "dsb ish",
            "msr mair_el1, {mair}",
            "msr tcr_el1, {tcr}",
            "msr ttbr0_el1, {ttbr0}",
            "msr ttbr1_el1, {ttbr1}",
            "isb",
            "tlbi vmalle1",
            "dsb ish",
            "isb",
            "mrs {tmp}, sctlr_el1",
            "orr {tmp}, {tmp}, {set}",
            "bic {tmp}, {tmp}, {clear}",
            "msr sctlr_el1, {tmp}",
            "isb",
            mair = in(reg) MAIR_VALUE,
            tcr = in(reg) tcr_value(),
            ttbr0 = in(reg) ttbr0,
            ttbr1 = in(reg) ttbr1,
            // SPAN: leave PAN alone on exceptions, so syscalls can read
            // and write the program memory they're handed.
            set = in(reg) (1u64 << 0) | (1 << 2) | (1 << 12) | (1 << 23),   // M, C, I, SPAN
            clear = in(reg) (1u64 << 1) | (1 << 19),            // A (alignment checks), WXN
            tmp = out(reg) _,
        );
    }
}

pub fn is_enabled() -> bool {
    let sctlr: u64;
    unsafe { asm!("mrs {0}, sctlr_el1", out(reg) sctlr) };
    sctlr & 1 != 0
}

/// Hardware walk via `AT S1E1R`: what the MMU actually does with `va` for
/// an EL1 read. Returns the physical address, or the fault status code.
pub fn hw_translate(va: usize) -> Result<usize, u64> {
    let par: u64;
    unsafe {
        asm!("at s1e1r, {va}", "isb", "mrs {par}, par_el1", va = in(reg) va, par = out(reg) par);
    }
    if par & 1 != 0 {
        Err((par >> 1) & 0x3f)
    } else {
        Ok((par & ADDR_MASK) as usize | (va & (PAGE_SIZE - 1)))
    }
}

/// Makes freshly written instructions in `start..start+len` visible to
/// instruction fetch. Needed after copying code into memory (program
/// loaders) now that the caches are on.
pub fn sync_icache(start: usize, len: usize) {
    let ctr: u64;
    unsafe { asm!("mrs {0}, ctr_el0", out(reg) ctr) };
    // Smallest line sizes, encoded as log2(words).
    let dline = 4usize << ((ctr >> 16) & 0xf);
    let iline = 4usize << (ctr & 0xf);
    let end = start + len;
    let mut addr = start & !(dline - 1);
    while addr < end {
        unsafe { asm!("dc cvau, {0}", in(reg) addr) };
        addr += dline;
    }
    unsafe { asm!("dsb ish") };
    let mut addr = start & !(iline - 1);
    while addr < end {
        unsafe { asm!("ic ivau, {0}", in(reg) addr) };
        addr += iline;
    }
    unsafe { asm!("dsb ish", "isb") };
}
