//! Builds a FAT disk image for KOS from host directories.
//!
//!     kos-mkdisk <image> <size-MiB> <dir>...
//!     kos-mkdisk --put <image> <dir> <dest>
//!
//! The first form creates (or overwrites) `image`, formats it with the same
//! `fatfs` crate the kernel mounts it with, then copies the contents of each
//! `dir` into the root of the volume. Later directories win on name clashes.
//!
//! `--put` copies `dir`'s contents into folder `dest` of an existing image,
//! replacing files with the same names and leaving everything else alone,
//! so files written from inside KOS survive.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::process::exit;

use fatfs::{DefaultTimeProvider, FileSystem, FormatVolumeOptions, FsOptions, LossyOemCpConverter, StdIoWrapper};

type Dir<'a> = fatfs::Dir<'a, StdIoWrapper<fs::File>, DefaultTimeProvider, LossyOemCpConverter>;

fn copy_tree(host: &Path, dir: &Dir, depth: usize) -> io::Result<()> {
    let mut entries: Vec<_> = fs::read_dir(host)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name();
        let name = name.to_str().expect("non-UTF-8 file name");
        if name == ".DS_Store" || name.starts_with("._") {
            continue;
        }
        let indent = "  ".repeat(depth + 1);
        if entry.file_type()?.is_dir() {
            println!("{indent}{name}/");
            let sub = dir.create_dir(name).map_err(fat_err)?;
            copy_tree(&entry.path(), &sub, depth + 1)?;
        } else {
            let data = fs::read(entry.path())?;
            println!("{indent}{name} ({} bytes)", data.len());
            let mut file = dir.create_file(name).map_err(fat_err)?;
            file.truncate().map_err(fat_err)?;
            file.write_all(&data)?;
        }
    }
    Ok(())
}

fn fat_err<E: std::fmt::Debug>(e: fatfs::Error<E>) -> io::Error {
    io::Error::other(format!("{e:?}"))
}

/// `--put`: copy `dir` into `dest` (created if needed) in an existing image.
fn put(image: &str, dir: &str, dest: &str) -> io::Result<()> {
    let file = OpenOptions::new().read(true).write(true).open(image)?;
    let fs = FileSystem::new(StdIoWrapper::new(file), FsOptions::new()).map_err(fat_err)?;
    {
        let mut target = fs.root_dir();
        for part in dest.split('/').filter(|p| !p.is_empty()) {
            // Opens the folder if it's already there.
            target = target.create_dir(part).map_err(fat_err)?;
        }
        println!("{image}: into /{}:", dest.trim_matches('/'));
        copy_tree(Path::new(dir), &target, 0)?;
    }
    fs.unmount().map_err(fat_err)?;
    Ok(())
}

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 5 && args[1] == "--put" {
        return put(&args[2], &args[3], &args[4]);
    }
    if args.len() < 3 {
        eprintln!("usage: kos-mkdisk <image> <size-MiB> <dir>...\n       kos-mkdisk --put <image> <dir> <dest>");
        exit(2);
    }
    let image = &args[1];
    let size_mib: u64 = args[2].parse().expect("size must be a number of MiB");

    let file = OpenOptions::new().read(true).write(true).create(true).truncate(true).open(image)?;
    file.set_len(size_mib * 1024 * 1024)?;
    let mut storage = StdIoWrapper::new(file);
    fatfs::format_volume(&mut storage, FormatVolumeOptions::new().volume_label(*b"KOS        ")).map_err(fat_err)?;

    let fs = FileSystem::new(storage, FsOptions::new()).map_err(fat_err)?;
    println!("{image} ({size_mib} MiB, {:?}):", fs.fat_type());
    for dir in &args[3..] {
        copy_tree(Path::new(dir), &fs.root_dir(), 0)?;
    }
    fs.unmount().map_err(fat_err)?;
    Ok(())
}
