//! SimpleFS: a small from-scratch filesystem.
//!
//! Layout (block == 512-byte sector, 1:1 with virtio-blk sectors):
//!   block 0            : superblock
//!   [inode_table_start..) : fixed-size inode table, INODE_SIZE bytes each
//!   [bitmap_start..)      : free-block bitmap, 1 bit per data block
//!   [data_start..)        : data blocks (file contents / directory entries)
//!
//! Inode 0 is always the root directory.
//! Every directory's data is just a flat array of Dirent records, including
//! synthetic "." (self) and ".." (parent) entries created at mkdir time.
//! That means path resolution never needs special-case handling for . or ..
//! -- they're just names you can look up like any other.
//!
//! Limitations (fine for a first pass, worth knowing):
//!   - No append: write_file always overwrites the full contents.
//!   - No timestamps, permissions, symlinks, or journaling.
//!   - Max file size: 8 direct blocks + 128 via one indirect block
//!     = 4096 + 65536 = 69632 bytes (~68 KiB). Add double-indirect later
//!     if you need more.
//!   - Directory data grows the same way as file data (no dedicated
//!     directory block limit), but there's no compaction of freed dirent
//!     slots beyond simple reuse-on-scan.
//!   - Not thread-safe / not reentrant: exactly one owner (fs_task) is
//!     expected to call these methods, one request at a time.

use crate::drivers::virtio::blk::BlkDevice;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::convert::TryInto;

use crate::filesys::backend::{DirEntryInfo, FsError};

pub const BLOCK_SIZE: usize = 512;
pub const DIRECT_PTRS: usize = 8;
pub const INODE_SIZE: usize = 64;
pub const DIRENT_SIZE: usize = 32;
pub const NAME_CAP: usize = 27;
pub const PTRS_PER_INDIRECT: usize = BLOCK_SIZE / 4; // 128

pub const MODE_FREE: u32 = 0;
pub const MODE_FILE: u32 = 1;
pub const MODE_DIR: u32 = 2;

const MAGIC: [u8; 4] = *b"SFS1";
const DEFAULT_INODE_COUNT: u32 = 256;

// ---- On-disk structures --------------------------------------------------

#[derive(Clone, Copy)]
struct Superblock {
    total_blocks: u32,
    inode_count: u32,
    inode_table_start: u32,
    inode_table_blocks: u32,
    bitmap_start: u32,
    bitmap_blocks: u32,
    data_start: u32,
    data_blocks: u32,
}

impl Superblock {
    fn to_bytes(&self, out: &mut [u8; BLOCK_SIZE]) {
        out.fill(0);
        out[0..4].copy_from_slice(&MAGIC);
        out[4..8].copy_from_slice(&self.total_blocks.to_le_bytes());
        out[8..12].copy_from_slice(&self.inode_count.to_le_bytes());
        out[12..16].copy_from_slice(&self.inode_table_start.to_le_bytes());
        out[16..20].copy_from_slice(&self.inode_table_blocks.to_le_bytes());
        out[20..24].copy_from_slice(&self.bitmap_start.to_le_bytes());
        out[24..28].copy_from_slice(&self.bitmap_blocks.to_le_bytes());
        out[28..32].copy_from_slice(&self.data_start.to_le_bytes());
        out[32..36].copy_from_slice(&self.data_blocks.to_le_bytes());
    }

    fn from_bytes(b: &[u8; BLOCK_SIZE]) -> Option<Self> {
        if b[0..4] != MAGIC {
            return None;
        }
        let u = |r: core::ops::Range<usize>| u32::from_le_bytes(b[r].try_into().unwrap());
        Some(Self {
            total_blocks: u(4..8),
            inode_count: u(8..12),
            inode_table_start: u(12..16),
            inode_table_blocks: u(16..20),
            bitmap_start: u(20..24),
            bitmap_blocks: u(24..28),
            data_start: u(28..32),
            data_blocks: u(32..36),
        })
    }
}

#[derive(Clone, Copy)]
struct Inode {
    mode: u32,
    size: u32,
    direct: [u32; DIRECT_PTRS],
    indirect: u32,
}

impl Inode {
    fn free() -> Self {
        Self { mode: MODE_FREE, size: 0, direct: [0; DIRECT_PTRS], indirect: 0 }
    }

    fn to_bytes(&self, out: &mut [u8; INODE_SIZE]) {
        out.fill(0);
        out[0..4].copy_from_slice(&self.mode.to_le_bytes());
        out[4..8].copy_from_slice(&self.size.to_le_bytes());
        for i in 0..DIRECT_PTRS {
            let off = 8 + i * 4;
            out[off..off + 4].copy_from_slice(&self.direct[i].to_le_bytes());
        }
        let ioff = 8 + DIRECT_PTRS * 4;
        out[ioff..ioff + 4].copy_from_slice(&self.indirect.to_le_bytes());
    }

    fn from_bytes(b: &[u8; INODE_SIZE]) -> Self {
        let u = |r: core::ops::Range<usize>| u32::from_le_bytes(b[r].try_into().unwrap());
        let mut direct = [0u32; DIRECT_PTRS];
        for i in 0..DIRECT_PTRS {
            direct[i] = u(8 + i * 4..12 + i * 4);
        }
        let ioff = 8 + DIRECT_PTRS * 4;
        Self { mode: u(0..4), size: u(4..8), direct, indirect: u(ioff..ioff + 4) }
    }

    fn blocks_used(&self) -> usize {
        (self.size as usize + BLOCK_SIZE - 1) / BLOCK_SIZE
    }
}

#[derive(Clone)]
struct Dirent {
    name: String,
    inode: u32,
}

impl Dirent {
    fn to_bytes(&self, out: &mut [u8; DIRENT_SIZE]) {
        out.fill(0);
        let bytes = self.name.as_bytes();
        let len = bytes.len().min(NAME_CAP);
        out[0] = len as u8;
        out[1..1 + len].copy_from_slice(&bytes[..len]);
        out[1 + NAME_CAP..1 + NAME_CAP + 4].copy_from_slice(&self.inode.to_le_bytes());
    }

    /// Returns None for an empty (unused) slot.
    fn from_bytes(b: &[u8; DIRENT_SIZE]) -> Option<Self> {
        let len = b[0] as usize;
        if len == 0 {
            return None;
        }
        let name = String::from_utf8_lossy(&b[1..1 + len]).to_string();
        let inode = u32::from_le_bytes(b[1 + NAME_CAP..1 + NAME_CAP + 4].try_into().unwrap());
        Some(Self { name, inode })
    }
}

// ---- Filesystem -----------------------------------------------------------

pub struct FileSystem {
    blk: BlkDevice,
    sb: Superblock,
    bitmap: Vec<u8>, // cached; bit i corresponds to data block (data_start + i)
}

impl FileSystem {
    /// Formats a fresh filesystem over the whole device and mounts it.
    pub fn format(blk: BlkDevice, total_blocks: u32) -> Self {
        let inode_count = DEFAULT_INODE_COUNT;
        let inode_table_blocks =
            ((inode_count as usize * INODE_SIZE) + BLOCK_SIZE - 1) as u32 / BLOCK_SIZE as u32;
        // Size the bitmap to cover the whole disk (a small overestimate of
        // the true data-block count) to avoid a circular size dependency.
        let bitmap_blocks = ((total_blocks as usize + 7) / 8 + BLOCK_SIZE - 1) as u32
            / BLOCK_SIZE as u32;
        let data_start = 1 + inode_table_blocks + bitmap_blocks;
        let data_blocks = total_blocks.saturating_sub(data_start);

        let sb = Superblock {
            total_blocks,
            inode_count,
            inode_table_start: 1,
            inode_table_blocks,
            bitmap_start: 1 + inode_table_blocks,
            bitmap_blocks,
            data_start,
            data_blocks,
        };

        let mut fs = FileSystem { blk, sb, bitmap: alloc::vec![0u8; (bitmap_blocks as usize) * BLOCK_SIZE] };

        // Zero the inode table.
        let zero = [0u8; BLOCK_SIZE];
        for i in 0..inode_table_blocks {
            fs.blk.write_sector((sb.inode_table_start + i) as u64, &zero);
        }
        // Zero + flush the bitmap.
        for i in 0..bitmap_blocks {
            fs.blk.write_sector((sb.bitmap_start + i) as u64, &zero);
        }
        // Write superblock.
        let mut sbbuf = [0u8; BLOCK_SIZE];
        fs.sb.to_bytes(&mut sbbuf);
        fs.blk.write_sector(0, &sbbuf);

        // Create root directory as inode 0.
        let root_block = fs.alloc_block().expect("no space to create root dir");
        let root = Inode { mode: MODE_DIR, size: 0, direct: {
            let mut d = [0u32; DIRECT_PTRS];
            d[0] = root_block;
            d
        }, indirect: 0 };
        fs.write_inode(0, &root);
        // Root's ".." points to itself.
        fs.append_dirent(0, &Dirent { name: ".".into(), inode: 0 });
        fs.append_dirent(0, &Dirent { name: "..".into(), inode: 0 });

        fs
    }

    /// Tries to mount an existing filesystem; formats a fresh one if the
    /// superblock magic doesn't match (e.g. first boot on a blank disk).
    pub fn mount_or_format(blk: BlkDevice) -> Self {
        let mut buf = [0u8; BLOCK_SIZE];
        blk.read_sector(0, &mut buf);
        if let Some(sb) = Superblock::from_bytes(&buf) {
            let mut bitmap = alloc::vec![0u8; (sb.bitmap_blocks as usize) * BLOCK_SIZE];
            for i in 0..sb.bitmap_blocks {
                let mut sector = [0u8; BLOCK_SIZE];
                blk.read_sector((sb.bitmap_start + i) as u64, &mut sector);
                let off = i as usize * BLOCK_SIZE;
                bitmap[off..off + BLOCK_SIZE].copy_from_slice(&sector);
            }
            FileSystem { blk, sb, bitmap }
        } else {
            let total = blk.capacity_sectors().min(u32::MAX as u64) as u32;
            let total = if total == 0 { 65536 } else { total }; // fallback if config unreadable
            Self::format(blk, total)
        }
    }

    // ---- low-level block / inode helpers ----

    fn read_block(&self, block: u32, buf: &mut [u8; BLOCK_SIZE]) {
        self.blk.read_sector(block as u64, buf);
    }

    fn write_block(&self, block: u32, buf: &[u8; BLOCK_SIZE]) {
        self.blk.write_sector(block as u64, buf);
    }

    fn read_inode(&self, n: u32) -> Inode {
        let byte_off = n as usize * INODE_SIZE;
        let block = self.sb.inode_table_start + (byte_off / BLOCK_SIZE) as u32;
        let in_block = byte_off % BLOCK_SIZE;
        let mut buf = [0u8; BLOCK_SIZE];
        self.read_block(block, &mut buf);
        let mut ib = [0u8; INODE_SIZE];
        ib.copy_from_slice(&buf[in_block..in_block + INODE_SIZE]);
        Inode::from_bytes(&ib)
    }

    fn write_inode(&mut self, n: u32, inode: &Inode) {
        let byte_off = n as usize * INODE_SIZE;
        let block = self.sb.inode_table_start + (byte_off / BLOCK_SIZE) as u32;
        let in_block = byte_off % BLOCK_SIZE;
        let mut buf = [0u8; BLOCK_SIZE];
        self.read_block(block, &mut buf);
        let mut ib = [0u8; INODE_SIZE];
        inode.to_bytes(&mut ib);
        buf[in_block..in_block + INODE_SIZE].copy_from_slice(&ib);
        self.write_block(block, &buf);
    }

    fn alloc_inode(&mut self) -> Result<u32, FsError> {
        for n in 1..self.sb.inode_count {
            if self.read_inode(n).mode == MODE_FREE {
                return Ok(n);
            }
        }
        Err(FsError::NoSpace)
    }

    fn alloc_block(&mut self) -> Option<u32> {
        for bit in 0..self.sb.data_blocks as usize {
            let byte = bit / 8;
            let mask = 1u8 << (bit % 8);
            if self.bitmap[byte] & mask == 0 {
                self.bitmap[byte] |= mask;
                self.flush_bitmap_byte(byte);
                return Some(self.sb.data_start + bit as u32);
            }
        }
        None
    }

    fn free_block(&mut self, block: u32) {
        let bit = (block - self.sb.data_start) as usize;
        let byte = bit / 8;
        let mask = 1u8 << (bit % 8);
        self.bitmap[byte] &= !mask;
        self.flush_bitmap_byte(byte);
    }

    fn flush_bitmap_byte(&mut self, byte: usize) {
        let sector_idx = byte / BLOCK_SIZE;
        let sector_off = sector_idx * BLOCK_SIZE;
        let mut buf = [0u8; BLOCK_SIZE];
        buf.copy_from_slice(&self.bitmap[sector_off..sector_off + BLOCK_SIZE]);
        self.write_block(self.sb.bitmap_start + sector_idx as u32, &buf);
    }

    /// Block numbers backing an inode's first `count` blocks (allocating
    /// an indirect block on demand if needed, but not the data blocks
    /// themselves -- callers allocate those separately when growing).
    fn block_ptr(&mut self, inode: &mut Inode, idx: usize) -> Option<u32> {
        if idx < DIRECT_PTRS {
            let b = inode.direct[idx];
            if b != 0 { Some(b) } else { None }
        } else {
            if inode.indirect == 0 {
                return None;
            }
            let mut buf = [0u8; BLOCK_SIZE];
            self.read_block(inode.indirect, &mut buf);
            let off = (idx - DIRECT_PTRS) * 4;
            let b = u32::from_le_bytes(buf[off..off + 4].try_into().unwrap());
            if b != 0 { Some(b) } else { None }
        }
    }

    fn set_block_ptr(&mut self, inode: &mut Inode, idx: usize, block: u32) {
        if idx < DIRECT_PTRS {
            inode.direct[idx] = block;
        } else {
            if inode.indirect == 0 {
                inode.indirect = self.alloc_block().expect("no space for indirect block");
                let zero = [0u8; BLOCK_SIZE];
                self.write_block(inode.indirect, &zero);
            }
            let mut buf = [0u8; BLOCK_SIZE];
            self.read_block(inode.indirect, &mut buf);
            let off = (idx - DIRECT_PTRS) * 4;
            buf[off..off + 4].copy_from_slice(&block.to_le_bytes());
            self.write_block(inode.indirect, &buf);
        }
    }

    fn read_inode_data(&self, inode: &Inode) -> Vec<u8> {
        let mut out = Vec::with_capacity(inode.size as usize);
        let blocks = inode.blocks_used();
        let mut inode_mut = *inode; // block_ptr needs &mut self but only reads here
        for i in 0..blocks {
            // Safety note: read-only traversal; using a scratch mutable copy
            // since block_ptr's signature is shared with the write path.
            let ptr = {
                // manual inline of block_ptr's read-only path to avoid
                // requiring &mut self on a read
                if i < DIRECT_PTRS {
                    inode_mut.direct[i]
                } else if inode_mut.indirect != 0 {
                    let mut buf = [0u8; BLOCK_SIZE];
                    self.read_block(inode_mut.indirect, &mut buf);
                    let off = (i - DIRECT_PTRS) * 4;
                    u32::from_le_bytes(buf[off..off + 4].try_into().unwrap())
                } else {
                    0
                }
            };
            let mut buf = [0u8; BLOCK_SIZE];
            if ptr != 0 {
                self.read_block(ptr, &mut buf);
            }
            let remaining = inode.size as usize - out.len();
            let take = remaining.min(BLOCK_SIZE);
            out.extend_from_slice(&buf[..take]);
        }
        out
    }

    fn write_inode_data(&mut self, n: u32, inode: &mut Inode, data: &[u8]) {
        let old_blocks = inode.blocks_used();
        let new_blocks = (data.len() + BLOCK_SIZE - 1) / BLOCK_SIZE;

        for i in 0..new_blocks {
            let block = self.block_ptr(inode, i).unwrap_or_else(|| {
                let b = self.alloc_block().expect("out of disk space");
                self.set_block_ptr(inode, i, b);
                b
            });
            let mut buf = [0u8; BLOCK_SIZE];
            let start = i * BLOCK_SIZE;
            let end = (start + BLOCK_SIZE).min(data.len());
            buf[..end - start].copy_from_slice(&data[start..end]);
            self.write_block(block, &buf);
        }
        // Free any now-unused trailing blocks.
        for i in new_blocks..old_blocks {
            if let Some(b) = self.block_ptr(inode, i) {
                self.free_block(b);
                if i < DIRECT_PTRS {
                    inode.direct[i] = 0;
                }
            }
        }
        // If we no longer need indirection at all, free that block too.
        if new_blocks <= DIRECT_PTRS && inode.indirect != 0 {
            self.free_block(inode.indirect);
            inode.indirect = 0;
        }
        inode.size = data.len() as u32;
        self.write_inode(n, inode);
    }

    // ---- directory helpers ----

    fn read_dirents(&self, inode: &Inode) -> Vec<Dirent> {
        let raw = self.read_inode_data(inode);
        raw.chunks(DIRENT_SIZE)
            .filter_map(|chunk| {
                if chunk.len() < DIRENT_SIZE {
                    return None;
                }
                let mut arr = [0u8; DIRENT_SIZE];
                arr.copy_from_slice(chunk);
                Dirent::from_bytes(&arr)
            })
            .collect()
    }

    fn append_dirent(&mut self, dir_inode_num: u32, entry: &Dirent) {
        let inode = self.read_inode(dir_inode_num);
        let mut raw = self.read_inode_data(&inode);
        let mut eb = [0u8; DIRENT_SIZE];
        entry.to_bytes(&mut eb);
        raw.extend_from_slice(&eb);
        let mut inode = inode;
        self.write_inode_data(dir_inode_num, &mut inode, &raw);
    }

    fn remove_dirent(&mut self, dir_inode_num: u32, name: &str) {
        let inode = self.read_inode(dir_inode_num);
        let entries = self.read_dirents(&inode);
        let mut raw = Vec::with_capacity(entries.len() * DIRENT_SIZE);
        for e in entries.iter().filter(|e| e.name != name) {
            let mut eb = [0u8; DIRENT_SIZE];
            e.to_bytes(&mut eb);
            raw.extend_from_slice(&eb);
        }
        let mut inode = inode;
        self.write_inode_data(dir_inode_num, &mut inode, &raw);
    }

    fn lookup_child(&self, dir_inode_num: u32, name: &str) -> Result<u32, FsError> {
        let inode = self.read_inode(dir_inode_num);
        if inode.mode != MODE_DIR {
            return Err(FsError::NotADirectory);
        }
        self.read_dirents(&inode)
            .into_iter()
            .find(|e| e.name == name)
            .map(|e| e.inode)
            .ok_or(FsError::NotFound)
    }

    fn resolve(&self, cwd: u32, path: &str) -> Result<u32, FsError> {
        let path = path.trim();
        if path.is_empty() {
            return Err(FsError::InvalidPath);
        }
        let mut cur = if path.starts_with('/') { 0 } else { cwd };
        for comp in path.split('/').filter(|c| !c.is_empty()) {
            cur = self.lookup_child(cur, comp)?;
        }
        Ok(cur)
    }

    /// Resolves the parent directory and final component name of a path,
    /// for operations that need to create or remove that final entry.
    fn resolve_parent(&self, cwd: u32, path: &str) -> Result<(u32, String), FsError> {
        let path = path.trim().trim_end_matches('/');
        if path.is_empty() {
            return Err(FsError::InvalidPath);
        }
        let start = if path.starts_with('/') { 0 } else { cwd };
        let comps: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
        if comps.is_empty() {
            return Err(FsError::InvalidPath);
        }
        let (dirs, last) = comps.split_at(comps.len() - 1);
        let mut cur = start;
        for d in dirs {
            cur = self.lookup_child(cur, d)?;
        }
        Ok((cur, last[0].to_string()))
    }

    // ---- public operations, called from fs_task via the FsBackend trait ----
    //
    // cwd is now an absolute path string, not an inode number -- see
    // filesys/backend.rs for why. Every call joins cwd+path into a single
    // absolute path and resolves it from root (inode 0); the numeric-inode
    // fast path is gone, but that's a non-issue at these filesystem sizes.

    pub fn list_dir(&self, cwd: &str, path: &str) -> Result<Vec<DirEntryInfo>, FsError> {
        let full = crate::filesys::backend::join_path(cwd, path);
        let target = self.resolve(0, &full)?;
        let inode = self.read_inode(target);
        if inode.mode != MODE_DIR {
            return Err(FsError::NotADirectory);
        }
        Ok(self
            .read_dirents(&inode)
            .into_iter()
            .map(|e| {
                let child = self.read_inode(e.inode);
                DirEntryInfo { name: e.name, is_dir: child.mode == MODE_DIR, size: child.size }
            })
            .collect())
    }

    pub fn change_dir(&self, cwd: &str, path: &str) -> Result<String, FsError> {
        let full = crate::filesys::backend::join_path(cwd, path);
        let target = self.resolve(0, &full)?;
        if self.read_inode(target).mode != MODE_DIR {
            return Err(FsError::NotADirectory);
        }
        Ok(full)
    }

    pub fn read_file(&self, cwd: &str, path: &str) -> Result<Vec<u8>, FsError> {
        let full = crate::filesys::backend::join_path(cwd, path);
        let target = self.resolve(0, &full)?;
        let inode = self.read_inode(target);
        if inode.mode != MODE_FILE {
            return Err(FsError::NotAFile);
        }
        Ok(self.read_inode_data(&inode))
    }

    pub fn write_file(&mut self, cwd: &str, path: &str, data: &[u8]) -> Result<(), FsError> {
        let full = crate::filesys::backend::join_path(cwd, path);
        let target = match self.resolve(0, &full) {
            Ok(n) => n,
            Err(FsError::NotFound) => self.create(0, &full, MODE_FILE)?,
            Err(e) => return Err(e),
        };
        let mut inode = self.read_inode(target);
        if inode.mode != MODE_FILE {
            return Err(FsError::NotAFile);
        }
        self.write_inode_data(target, &mut inode, data);
        Ok(())
    }

    pub fn mkdir(&mut self, cwd: &str, path: &str) -> Result<(), FsError> {
        let full = crate::filesys::backend::join_path(cwd, path);
        self.create(0, &full, MODE_DIR)?;
        Ok(())
    }

    pub fn create_file(&mut self, cwd: &str, path: &str) -> Result<(), FsError> {
        let full = crate::filesys::backend::join_path(cwd, path);
        self.create(0, &full, MODE_FILE)?;
        Ok(())
    }

    fn create(&mut self, cwd: u32, path: &str, mode: u32) -> Result<u32, FsError> {
        let (parent, name) = self.resolve_parent(cwd, path)?;
        if name.len() > NAME_CAP {
            return Err(FsError::InvalidPath);
        }
        if self.lookup_child(parent, &name).is_ok() {
            return Err(FsError::AlreadyExists);
        }
        let n = self.alloc_inode()?;
        let inode = Inode { mode, size: 0, direct: [0; DIRECT_PTRS], indirect: 0 };
        self.write_inode(n, &inode);
        self.append_dirent(parent, &Dirent { name, inode: n });

        if mode == MODE_DIR {
            self.append_dirent(n, &Dirent { name: ".".into(), inode: n });
            self.append_dirent(n, &Dirent { name: "..".into(), inode: parent });
        }
        Ok(n)
    }

    pub fn remove(&mut self, cwd: &str, path: &str) -> Result<(), FsError> {
        let full = crate::filesys::backend::join_path(cwd, path);
        let (parent, name) = self.resolve_parent(0, &full)?;
        let target = self.lookup_child(parent, &name)?;
        let inode = self.read_inode(target);

        if inode.mode == MODE_DIR {
            let entries = self.read_dirents(&inode);
            if entries.iter().any(|e| e.name != "." && e.name != "..") {
                return Err(FsError::NotEmpty);
            }
        }

        // Free all data blocks (including the indirect block itself).
        let blocks = inode.blocks_used();
        let mut inode_mut = inode;
        for i in 0..blocks {
            if let Some(b) = self.block_ptr(&mut inode_mut, i) {
                self.free_block(b);
            }
        }
        if inode.indirect != 0 {
            self.free_block(inode.indirect);
        }

        self.write_inode(target, &Inode::free());
        self.remove_dirent(parent, &name);
        Ok(())
    }
}

impl crate::filesys::backend::FsBackend for FileSystem {
    fn mount_or_format(blk: BlkDevice) -> Self {
        FileSystem::mount_or_format(blk)
    }
    
    fn list_dir(&mut self, cwd: &str, path: &str) -> Result<Vec<DirEntryInfo>, FsError> {
        FileSystem::list_dir(self, cwd, path)
    }
    
    fn change_dir(&mut self, cwd: &str, path: &str) -> Result<String, FsError> {
        FileSystem::change_dir(self, cwd, path)
    }
    
    fn read_file(&mut self, cwd: &str, path: &str) -> Result<Vec<u8>, FsError> {
        FileSystem::read_file(self, cwd, path)
    }
    
    fn write_file(&mut self, cwd: &str, path: &str, data: &[u8]) -> Result<(), FsError> {
        FileSystem::write_file(self, cwd, path, data)
    }
    
    fn mkdir(&mut self, cwd: &str, path: &str) -> Result<(), FsError> {
        FileSystem::mkdir(self, cwd, path)
    }
    
    fn create_file(&mut self, cwd: &str, path: &str) -> Result<(), FsError> {
        FileSystem::create_file(self, cwd, path)
    }
    
    fn remove(&mut self, cwd: &str, path: &str) -> Result<(), FsError> {
        FileSystem::remove(self, cwd, path)
    }
}
