//! Builds KOS programs from sources anywhere on this computer.
//!
//!     kos-cc [-o <out.elf>] [--b64] <file.c|file.s>... [-- <clang args>...]
//!
//! C programs are linked with KOS's C library (userspace/libc), which is
//! on the include path, so they can `#include "sys.h"`, `"stdio.h"`,
//! `"string.h"`, `"stdlib.h"`, `"net.h"`, `"canvas.h"` and the rest. Assembly-only programs are built
//! without it, as `make user` does.
//!
//! The output is a plain ELF (`<first source>.elf` unless `-o` says
//! otherwise), which KOS runs with `run <name>.elf` once it's on the disk:
//! put it in disk/ before `make disk`, or use `kos-mkdisk --put`. `--b64`
//! also writes `<out>.b64`, the form the Programs window lists (from
//! /programs: disk/programs/ before `make disk`).
//!
//! The library is found at KOS_LIBC if that's set, otherwise in the
//! checkout this tool was built from. KOS_CLANG picks the compiler
//! (default `clang`, which needs `ld.lld` on the PATH).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{exit, Command};

const USAGE: &str = "usage: kos-cc [-o <out.elf>] [--b64] <file.c|file.s>... [-- <clang args>...]";

/// The flags `make user` builds programs with.
const CLANG_FLAGS: &[&str] = &[
    "-target", "aarch64-none-elf", "-fuse-ld=lld", "-nostdlib",
    "-fPIE", "-static", "-Wl,-pie", "-Wl,--image-base=0",
];
const C_FLAGS: &[&str] = &["-ffreestanding", "-Wall"];

fn fail(msg: impl AsRef<str>) -> ! {
    eprintln!("kos-cc: {}", msg.as_ref());
    exit(1);
}

fn libc_dir() -> PathBuf {
    let dir = match env::var_os("KOS_LIBC") {
        Some(dir) => PathBuf::from(dir),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../userspace/libc"),
    };
    if !dir.join("sys.h").is_file() {
        fail(format!("no KOS libc at {} (set KOS_LIBC to userspace/libc)", dir.display()));
    }
    dir.canonicalize().unwrap_or(dir)
}

/// The library's .c files: the folder's own and one level of subfolders.
fn libc_sources(dir: &Path) -> Vec<PathBuf> {
    let mut sources = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
    if let Ok(entries) = fs::read_dir(dir) {
        dirs.extend(entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
    }
    for d in dirs {
        let Ok(entries) = fs::read_dir(&d) else { continue };
        sources.extend(entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "c")));
    }
    sources.sort();
    sources
}

/// Base64 without line breaks, as KOS's loader reads it.
fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn main() {
    let mut output: Option<PathBuf> = None;
    let mut b64 = false;
    let mut sources: Vec<PathBuf> = Vec::new();
    let mut extra: Vec<String> = Vec::new();

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-o" => output = Some(args.next().unwrap_or_else(|| fail("-o needs a file name")).into()),
            "--b64" => b64 = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return;
            }
            "--" => {
                extra.extend(args.by_ref());
            }
            _ if arg.starts_with('-') => fail(format!("unknown option {arg}\n{USAGE}")),
            _ => sources.push(arg.into()),
        }
    }
    if sources.is_empty() {
        fail(USAGE);
    }
    for source in &sources {
        match source.extension().and_then(|x| x.to_str()) {
            Some("c" | "s" | "S") if source.is_file() => {}
            Some("c" | "s" | "S") => fail(format!("{}: no such file", source.display())),
            _ => fail(format!("{}: not a .c or .s file", source.display())),
        }
    }
    let output = output.unwrap_or_else(|| {
        let stem = sources[0].file_stem().unwrap_or_default();
        PathBuf::from(stem).with_extension("elf")
    });

    let has_c = sources.iter().any(|s| s.extension().is_some_and(|x| x == "c"));
    let clang = env::var("KOS_CLANG").unwrap_or_else(|_| "clang".to_string());
    let mut cmd = Command::new(&clang);
    cmd.args(CLANG_FLAGS);
    if has_c {
        let libc = libc_dir();
        cmd.args(C_FLAGS).arg("-I").arg(&libc);
        cmd.args(&sources).args(libc_sources(&libc));
    } else {
        cmd.args(&sources);
    }
    cmd.args(&extra).arg("-o").arg(&output);

    match cmd.status() {
        Ok(status) if status.success() => {}
        Ok(_) => fail("compiling failed"),
        Err(e) => fail(format!("can't run {clang}: {e}")),
    }
    println!("{}", output.display());

    if b64 {
        let elf = fs::read(&output).unwrap_or_else(|e| fail(format!("{}: {e}", output.display())));
        let mut path = output.into_os_string();
        path.push(".b64");
        let path = PathBuf::from(path);
        fs::write(&path, base64(&elf)).unwrap_or_else(|e| fail(format!("{}: {e}", path.display())));
        println!("{}", path.display());
    }
}
