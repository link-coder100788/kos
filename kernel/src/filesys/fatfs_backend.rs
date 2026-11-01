//! FAT32 filesystem backend using the `fatfs` crate over your existing
//! BlkDevice. Requires in Cargo.toml:
//!   fatfs = { version = "0.3.6", default-features = false, features = ["alloc", "lfn"] }
//!
//! On mount, this peeks at the raw boot-sector signature (bytes 510-511 ==
//! 0x55 0xAA) via a direct sector-0 read *before* constructing any fatfs
//! types, then either mounts or formats accordingly -- done this way
//! because fatfs::FileSystem::new() consumes its storage by value and does
//! not hand it back on error, so "try mount, reformat on failure" isn't
//! possible with the same storage handle. Peeking first avoids ever needing
//! that.

use alloc::string::String;
use alloc::vec::Vec;
use fatfs::{IoBase, IoError, Read as FatRead, Seek as FatSeek, SeekFrom, Write as FatWrite};
use crate::filesys::backend::{join_path, DirEntryInfo, FsBackend, FsError};
use crate::println;
use crate::drivers::virtio::blk::BlkDevice;

#[derive(Debug)]
pub struct BlkIoError;

/// Adapts BlkDevice's 512-byte sector read/write into the arbitrary
/// byte-range Read/Write/Seek that fatfs expects, tracking a byte cursor
/// and doing read-modify-write for any access that isn't sector-aligned.
pub struct BlkStorage {
    blk: BlkDevice,
    pos: u64,
    capacity_bytes: u64,
}

impl BlkStorage {
    pub fn new(blk: BlkDevice) -> Self {
        let capacity_bytes = blk.capacity_sectors() * 512;
        Self { blk, pos: 0, capacity_bytes }
    }
}

impl IoError for BlkIoError {
    fn is_interrupted(&self) -> bool {
        false
    }

    fn new_unexpected_eof_error() -> Self {
        BlkIoError {}
    }

    fn new_write_zero_error() -> Self {
        BlkIoError {}
    }
}

impl IoBase for BlkStorage {
    type Error = BlkIoError;
}

impl FatRead for BlkStorage {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        let mut done = 0usize;
        while done < buf.len() {
            let abs = self.pos + done as u64;
            if abs >= self.capacity_bytes {
                break;
            }
            let sector = abs / 512;
            let offset = (abs % 512) as usize;
            let mut sec_buf = [0u8; 512];
            if !self.blk.read_sector(sector, &mut sec_buf) {
                return Err(BlkIoError);
            }
            let take = (512 - offset).min(buf.len() - done);
            buf[done..done + take].copy_from_slice(&sec_buf[offset..offset + take]);
            done += take;
        }
        self.pos += done as u64;
        Ok(done)
    }
}

impl FatWrite for BlkStorage {
    fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        let mut done = 0usize;
        while done < buf.len() {
            let abs = self.pos + done as u64;
            let sector = abs / 512;
            let offset = (abs % 512) as usize;
            let take = (512 - offset).min(buf.len() - done);
            let mut sec_buf = [0u8; 512];
            // Only need to read the existing sector first if this write
            // doesn't cover the whole thing -- otherwise we'd overwrite
            // untouched bytes with stale/zeroed buffer contents.
            if offset != 0 || take < 512 {
                if !self.blk.read_sector(sector, &mut sec_buf) {
                    return Err(BlkIoError);
                }
            }
            sec_buf[offset..offset + take].copy_from_slice(&buf[done..done + take]);
            if !self.blk.write_sector(sector, &sec_buf) {
                return Err(BlkIoError);
            }
            done += take;
        }
        self.pos += done as u64;
        Ok(done)
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl FatSeek for BlkStorage {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64, Self::Error> {
        let new_pos: i64 = match pos {
            SeekFrom::Start(n) => n as i64,
            SeekFrom::Current(n) => self.pos as i64 + n,
            SeekFrom::End(n) => self.capacity_bytes as i64 + n,
        };
        if new_pos < 0 {
            return Err(BlkIoError);
        }
        self.pos = new_pos as u64;
        Ok(self.pos)
    }
}

pub struct FatFsBackend {
    fs: fatfs::FileSystem<BlkStorage>,
}

impl FatFsBackend {
    fn root_relative(cwd: &str, path: &str) -> String {
        join_path(cwd, path).trim_start_matches('/').into()
    }
}

impl FsBackend for FatFsBackend {
    fn mount_or_format(blk: BlkDevice) -> Self {
        let looks_formatted = {
            let mut sector0 = [0u8; 512];
            blk.read_sector(0, &mut sector0);
            sector0[510] == 0x55 && sector0[511] == 0xAA
        };

        let mut storage = BlkStorage::new(blk);
        if !looks_formatted {
            log::warn!("no FAT boot signature; formatting a fresh volume");
            fatfs::format_volume(&mut storage, fatfs::FormatVolumeOptions::new())
                .expect("fatfs: format_volume failed");
        }

        // Boot-time only: if this fails even right after a fresh format,
        // something is fundamentally wrong with the backing device, so
        // treating it as fatal here (mirroring how BlkDevice::init()
        // failing is already fatal to fs_task) is reasonable -- this is
        // not a per-request path.
        let fs = fatfs::FileSystem::new(storage, fatfs::FsOptions::new())
            .expect("fatfs: mount failed even after format");
        log::info!("FAT volume mounted");
        FatFsBackend { fs }
    }

    fn list_dir(&mut self, cwd: &str, path: &str) -> Result<Vec<DirEntryInfo>, FsError> {
        let rel = Self::root_relative(cwd, path);
        let dir = if rel.is_empty() {
            self.fs.root_dir()
        } else {
            self.fs.root_dir().open_dir(&rel).map_err(|_| FsError::NotFound)?
        };
        let mut out = Vec::new();
        for entry in dir.iter() {
            let entry = entry.map_err(|_| FsError::Io)?;
            out.push(DirEntryInfo {
                name: entry.file_name(),
                is_dir: entry.is_dir(),
                size: entry.len() as u32,
            });
        }
        Ok(out)
    }

    fn change_dir(&mut self, cwd: &str, path: &str) -> Result<String, FsError> {
        let full = join_path(cwd, path);
        let rel = full.trim_start_matches('/');
        if !rel.is_empty() {
            self.fs.root_dir().open_dir(rel).map_err(|_| FsError::NotADirectory)?;
        }
        Ok(full)
    }

    fn read_file(&mut self, cwd: &str, path: &str) -> Result<Vec<u8>, FsError> {
        let rel = Self::root_relative(cwd, path);
        let mut file = self.fs.root_dir().open_file(&rel).map_err(|_| FsError::NotFound)?;
        let mut data = Vec::new();
        let mut buf = [0u8; 512];
        loop {
            let n = file.read(&mut buf).map_err(|_| FsError::Io)?;
            if n == 0 {
                break;
            }
            data.extend_from_slice(&buf[..n]);
        }
        Ok(data)
    }

    fn write_file(&mut self, cwd: &str, path: &str, data: &[u8]) -> Result<(), FsError> {
        let rel = Self::root_relative(cwd, path);
        let mut file = self.fs.root_dir().create_file(&rel).map_err(|_| FsError::Io)?;
        file.truncate().map_err(|_| FsError::Io)?;
        file.write_all(data).map_err(|_| FsError::Io)?;
        Ok(())
    }

    fn mkdir(&mut self, cwd: &str, path: &str) -> Result<(), FsError> {
        let rel = Self::root_relative(cwd, path);
        self.fs.root_dir().create_dir(&rel).map_err(|_| FsError::AlreadyExists)?;
        Ok(())
    }

    fn create_file(&mut self, cwd: &str, path: &str) -> Result<(), FsError> {
        let rel = Self::root_relative(cwd, path);
        self.fs.root_dir().create_file(&rel).map_err(|_| FsError::Io)?;
        Ok(())
    }

    fn remove(&mut self, cwd: &str, path: &str) -> Result<(), FsError> {
        let rel = Self::root_relative(cwd, path);
        self.fs.root_dir().remove(&rel).map_err(|_| FsError::NotFound)?;
        Ok(())
    }
}
