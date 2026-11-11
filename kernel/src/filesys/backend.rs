//! Common interface both filesystem backends implement. Swapping which one
//! is active is a one-line change in fs_task.rs (see the ActiveFs alias
//! there) -- everything else (fs_ipc, ShellCommand, the shell itself) talks
//! to this trait and doesn't know or care which backend is behind it.
//!
//! cwd is represented as an absolute path string ("/", "/foo/bar", ...)
//! rather than a numeric inode. That's the key unification: SimpleFs has
//! stable inode numbers it could use as a cheap cwd handle, but FAT has no
//! equivalent concept, so path strings are the only representation that
//! works identically for both. The cost is that every relative lookup
//! re-walks from the root each time instead of jumping straight to a
//! cached inode -- irrelevant at hobby-OS filesystem sizes.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::drivers::virtio::blk::BlkDevice;

#[derive(Debug, Clone, Copy)]
pub enum FsError {
    NotFound,
    NotADirectory,
    NotAFile,
    AlreadyExists,
    NotEmpty,
    NoSpace,
    InvalidPath,
    Io,
}

/// What programs see from `last_error`: the caller already knows the path,
/// so it isn't part of it.
impl core::fmt::Display for FsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            FsError::NotFound => "no such file or folder",
            FsError::NotADirectory => "not a folder",
            FsError::NotAFile => "is a folder",
            FsError::AlreadyExists => "already exists",
            FsError::NotEmpty => "folder isn't empty",
            FsError::NoSpace => "the disk is full",
            FsError::InvalidPath => "bad path",
            FsError::Io => "disk error",
        })
    }
}

pub struct DirEntryInfo {
    pub name: String,
    pub is_dir: bool,
    pub size: u32,
}

pub trait FsBackend {
    fn mount_or_format(blk: BlkDevice) -> Self
    where
        Self: Sized;

    fn list_dir(&mut self, cwd: &str, path: &str) -> Result<Vec<DirEntryInfo>, FsError>;
    /// Returns the new absolute cwd path on success.
    fn change_dir(&mut self, cwd: &str, path: &str) -> Result<String, FsError>;
    fn read_file(&mut self, cwd: &str, path: &str) -> Result<Vec<u8>, FsError>;
    fn write_file(&mut self, cwd: &str, path: &str, data: &[u8]) -> Result<(), FsError>;
    fn mkdir(&mut self, cwd: &str, path: &str) -> Result<(), FsError>;
    fn create_file(&mut self, cwd: &str, path: &str) -> Result<(), FsError>;
    fn remove(&mut self, cwd: &str, path: &str) -> Result<(), FsError>;
}

/// Joins a cwd (absolute path) with a possibly-relative path argument.
/// Does NOT canonicalize "." / ".." components -- both backends resolve
/// those structurally on their own (SimpleFs via real "." / ".." dirents,
/// FAT via the same convention on-disk), so leaving them in the string is
/// fine; it'll just get walked again next time.
pub fn join_path(cwd: &str, path: &str) -> String {
    let path = path.trim();
    if path.is_empty() || path == "." {
        return cwd.to_string();
    }
    if path.starts_with('/') {
        return path.to_string();
    }
    if cwd == "/" {
        alloc::format!("/{}", path)
    } else {
        alloc::format!("{}/{}", cwd, path)
    }
}
