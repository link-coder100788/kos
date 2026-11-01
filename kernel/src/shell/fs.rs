//! Filesystem shell commands (`ls`, `cd`, `cat`, ...). These are clients
//! of `fs_task` via `fs_call`.

use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use crate::filesys::fs_ipc::{fs_call, FsRequest, FsResponse};
use crate::println;
use crate::std::lock::mutex::Mutex;

pub enum ShellCommand {
    Ls(String),
    Cd(String),
    Cat(String),
    Mkdir(String),
    Rm(String),
    Write(String, String),
    /// A program and its arguments.
    Run(String, String),
    Bg(String, String),
    Decode(String, String),
    Encode(String, String),
    Open(String),
    Help,
}

// The shell's current-directory state lives here, with the caller
// (uart_read_task), not in fs_task -- fs_task stays a stateless server.
// If you add more shells/ttys later, give each its own cwd variable.
/// Empty means "/".
pub static SHELL_CWD: Mutex<String> = Mutex::new(String::new());

pub fn current_cwd() -> String {
    let cwd = SHELL_CWD.lock();
    if cwd.is_empty() { "/".to_string() } else { cwd.clone() }
}

fn set_cwd(path: String) {
    *SHELL_CWD.lock() = path;
}

/// Where system commands (userspace/cmd/) live on the disk.
pub const BIN_DIR: &str = "/bin";

/// `"prog arg1 arg2"` -> `("prog", "arg1 arg2")`.
fn split_command(s: &str) -> (&str, &str) {
    match s.split_once(' ') {
        Some((cmd, rest)) => (cmd, rest.trim()),
        None => (s, ""),
    }
}

/// The path of command `name` in `/bin` (`/bin/<name>.elf.b64`), if it's
/// there.
fn find_command(name: &str) -> Option<String> {
    if name.is_empty() || name.contains('/') {
        return None;
    }
    let file = alloc::format!("{}.elf.b64", name);
    match fs_call(FsRequest::List { cwd: "/".to_string(), path: BIN_DIR.to_string() }) {
        FsResponse::Listing(entries) if entries.iter().any(|e| !e.is_dir && e.name == file) => {
            Some(alloc::format!("{}/{}", BIN_DIR, file))
        }
        _ => None,
    }
}

impl ShellCommand {
    /// Never panics on malformed input -- worst case returns None (after
    /// printing why) so a shell typo can't take down the whole kernel.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let (cmd, rest) = match s.split_once(' ') {
            Some((c, r)) => (c, r.trim()),
            None => (s, ""),
        };
        match cmd {
            "ls" => Some(ShellCommand::Ls(if rest.is_empty() { ".".to_string() } else { rest.to_string() })),
            "cd" => Some(ShellCommand::Cd(if rest.is_empty() { "/".to_string() } else { rest.to_string() })),
            "cat" => {
                if rest.is_empty() {
                    println!("cat: missing path");
                    None
                } else {
                    Some(ShellCommand::Cat(rest.to_string()))
                }
            }
            "mkdir" => {
                if rest.is_empty() {
                    println!("mkdir: missing path");
                    None
                } else {
                    Some(ShellCommand::Mkdir(rest.to_string()))
                }
            }
            "rm" => {
                if rest.is_empty() {
                    println!("rm: missing path");
                    None
                } else {
                    Some(ShellCommand::Rm(rest.to_string()))
                }
            }
            "write" => match rest.split_once(' ') {
                Some((path, content)) => Some(ShellCommand::Write(path.to_string(), content.to_string())),
                None => {
                    println!("write: usage: write <path> <content>");
                    None
                }
            },
            "run" => {
                if rest.is_empty() {
                    println!("run: missing path");
                    None
                } else {
                    let (path, args) = split_command(rest);
                    Some(ShellCommand::Run(path.to_string(), args.to_string()))
                }
            },
            "bg" => {
                if rest.is_empty() {
                    println!("bg: missing path");
                    None
                } else {
                    let (path, args) = split_command(rest);
                    Some(ShellCommand::Bg(path.to_string(), args.to_string()))
                }
            },
            "decode" => match rest.split_once(' ') {
                Some((path1, path2)) => Some(ShellCommand::Decode(path1.to_string(), path2.to_string())),
                None => {
                    println!("decode: usage: decode <path> <path>");
                    None
                }
            },
            "encode" => match rest.split_once(' ') {
                Some((path1, path2)) => Some(ShellCommand::Encode(path1.to_string(), path2.to_string())),
                None => {
                    println!("encode: usage: encode <path> <path>");
                    None
                }
            },
            "open" => {
                if rest.is_empty() {
                    println!("open: missing path");
                    None
                } else {
                    Some(ShellCommand::Open(rest.to_string()))
                }
            },
            // Anything else may be a command in /bin.
            _ => match find_command(cmd) {
                Some(path) => Some(ShellCommand::Run(path, rest.to_string())),
                None => Some(ShellCommand::Help),
            },
        }
    }

    pub fn execute(&self) {
        let cwd = current_cwd();
        match self {
            ShellCommand::Help => {
                println!("Available commands:");
                println!("  ls <path>");
                println!("  cd <path>");
                println!("  cat <path>");
                println!("  mkdir <path>");
                println!("  rm <path>");
                println!("  write <path> <data>");
                println!("  run <path> [args]  run a program; the shell waits until it ends");
                println!("  bg <path> [args]   run a program in the background, without the console");
                println!("  <command> [args]   run /bin/<command>.elf.b64 (e.g. grep)");
                println!("  kill <id>          end a program (ids are in ps and the Task Manager)");
                println!("  decode <path> <path>");
                println!("  encode <path> <path>");
                println!("  open <image.bmp/png/jpeg/heic/mp4/mov>");
                println!("  k <build|run> <file>");
                println!("Network: net, ping <host>, dns <host or url>, fetch <url>, browse [url]");
                println!("Sound: beep, snd <hz> (snd 0 stops), play <notes> (e.g. play T140 O4 C D E F G2)");
                println!("Assistant: kai <request> (try `kai` alone), kai --debug <request>");
                println!("Debugging: log [target level], dmesg [count] [text], stats, ps");
                println!("System: mmu [addr], heap_remain, heap_test, load [seconds]");
                println!("  help");
            }
            ShellCommand::Ls(path) => match fs_call(FsRequest::List { cwd, path: path.clone() }) {
                FsResponse::Listing(entries) => {
                    println!("ls: ok: {}", entries.len());
                    for e in entries {
                        if e.name.starts_with(".") || e.name == "." || e.name == ".." { continue; }
                        println!("{}{}", e.name, if e.is_dir { "/" } else { "" });
                    }
                }
                FsResponse::Err(e) =>  { println!("ls: err: {:?}", e); },
                _ => { println!("ls: unexpected response"); },
            },
            ShellCommand::Cd(path) => match fs_call(FsRequest::ChangeDir { cwd: cwd.clone(), path: path.clone() }) {
                FsResponse::Path(new_cwd) => set_cwd(new_cwd),
                FsResponse::Err(e) => { println!("cd: err: {:?}", e); },
                _ => { println!("cd: unexpected response"); },
            },
            ShellCommand::Cat(path) => match fs_call(FsRequest::Read { cwd, path: path.clone() }) {
                FsResponse::Data(bytes) => match core::str::from_utf8(&bytes) {
                    Ok(s) =>  { println!("cat: +utf8: {}", s); },
                    Err(_) => { println!("cat: -utf8: <{} bytes of binary data>", bytes.len()); },
                },
                FsResponse::Err(e) => { println!("cat: err: {:?}", e); },
                _ => { println!("cat: unexpected response"); },
            },
            ShellCommand::Mkdir(path) => match fs_call(FsRequest::Mkdir { cwd, path: path.clone() }) {
                FsResponse::Ok => { println!("mkdir: ok"); },
                FsResponse::Err(e) => { println!("mkdir: err: {:?}", e); },
                _ => { println!("mkdir: unexpected response"); },
            },
            ShellCommand::Rm(path) => match fs_call(FsRequest::Remove { cwd, path: path.clone() }) {
                FsResponse::Ok => { println!("rm: ok"); },
                FsResponse::Err(e) => { println!("rm: err: {:?}", e); },
                _ => { println!("rm: unexpected response"); },
            },
            ShellCommand::Write(path, content) => {
                let data = Vec::from(content.as_bytes());
                match fs_call(FsRequest::Write { cwd, path: path.clone(), data }) {
                    FsResponse::Ok => { println!("write: ok"); },
                    FsResponse::Err(e) => { println!("write: err: {:?}", e); },
                    _ => { println!("write: unexpected response"); },
                }
            },
            ShellCommand::Run(path, args) => crate::loader::run_foreground(path.clone(), args.clone()),
            ShellCommand::Bg(path, args) => {
                crate::loader::run_background(path.clone(), args.clone());
                println!("bg: started {}", path);
            },
            ShellCommand::Decode(to, from) => match fs_call(FsRequest::Read { cwd: cwd.clone(), path: to.clone() }) {
                FsResponse::Data(bytes) => match core::str::from_utf8(&bytes) {
                    Ok(s) => {
                        let mut decoded = vec![0u8; s.len()];
                        STANDARD.decode_slice(s.as_bytes(), &mut decoded);
                        match fs_call(FsRequest::Write { cwd: cwd.clone(), path: from.clone(), data: decoded }) {
                            FsResponse::Ok => { println!("decode: write: ok"); },
                            FsResponse::Err(e) => { println!("decode: write: err: {:?}", e); },
                            _ => { println!("decode: write: unexpected response"); },
                        }
                    }
                    Err(e) => { println!("decode: needs b64 str"); }
                },
                FsResponse::Err(e) => { println!("decode: err: {:?}", e); },
                _ => { println!("decode: unexpected response"); },
            },
            ShellCommand::Encode(to, from) => match fs_call(FsRequest::Read { cwd: cwd.clone(), path: to.clone() }) {
                FsResponse::Data(bytes) => match core::str::from_utf8(&bytes) {
                    Ok(s) => { println!("encode: needs binary"); },
                    Err(_) => {
                        let mut encoded = vec![0u8; bytes.len()];
                        STANDARD.encode_slice(&bytes, &mut encoded);
                        match fs_call(FsRequest::Write { cwd: cwd.clone(), path: from.clone(), data: encoded }) {
                            FsResponse::Ok => { println!("encode: write: ok"); },
                            FsResponse::Err(e) => { println!("encode: write: err: {:?}", e); },
                            _ => { println!("encode: write: unexpected response"); },
                        }
                    },
                },
                FsResponse::Err(e) => { println!("encode: err: {:?}", e); },
                _ => { println!("encode: unexpected response"); },
            },
            ShellCommand::Open(path) => {
                let (_, ext) = path.split_once(".").unwrap_or(("", ""));
                match ext {
                    "bmp" => {
                        let bytes = match fs_call(FsRequest::Read { cwd: cwd.clone(), path: path.clone() }) {
                            FsResponse::Data(bytes) => bytes,
                            FsResponse::Err(e) => { println!("open: err: {:?}", e); return; }
                            _ => { println!("open: unexpected response"); return; }
                        };
                        match crate::ui::img::decode_bmp(&bytes) {
                            Ok(image) => {
                                let viewer = crate::ui::apps::image::ImageViewer::new(path.clone(), image);
                                crate::ui::window::open_app(alloc::boxed::Box::new(viewer));
                            }
                            Err(e) => println!("open: {}", e),
                        }
                    }
                    "png" => {
                        let bytes = match fs_call(FsRequest::Read { cwd: cwd.clone(), path: path.clone() }) {
                            FsResponse::Data(bytes) => bytes,
                            FsResponse::Err(e) => { println!("open: err: {:?}", e); return; }
                            _ => { println!("open: unexpected response"); return; }
                        };
                        match crate::ui::img::decode_png(&bytes) {
                            Ok(image) => {
                                let viewer = crate::ui::apps::image::ImageViewer::new(path.clone(), image);
                                crate::ui::window::open_app(alloc::boxed::Box::new(viewer));
                            }
                            Err(e) => println!("open: {}", e),
                        }
                    }
                    "jpeg" | "jpg" => {
                        let bytes = match fs_call(FsRequest::Read { cwd: cwd.clone(), path: path.clone() }) {
                            FsResponse::Data(bytes) => bytes,
                            FsResponse::Err(e) => { println!("open: err: {:?}", e); return; }
                            _ => { println!("open: unexpected response"); return; }
                        };
                        match crate::ui::img::decode_jpeg(&bytes) {
                           Ok(image) => {
                               let viewer = crate::ui::apps::image::ImageViewer::new(path.clone(), image);
                               crate::ui::window::open_app(alloc::boxed::Box::new(viewer));
                           }
                           Err(e) => println!("open: {}", e),
                       }
                    }
                    "heic" => {
                        let bytes = match fs_call(FsRequest::Read { cwd: cwd.clone(), path: path.clone() }) {
                            FsResponse::Data(bytes) => bytes,
                            FsResponse::Err(e) => { println!("open: err: {:?}", e); return; }
                            _ => { println!("open: unexpected response"); return; }
                        };
                        match crate::ui::img::decode_heic(&bytes) {
                            Ok(image) => {
                                let viewer = crate::ui::apps::image::ImageViewer::new(path.clone(), image);
                                crate::ui::window::open_app(alloc::boxed::Box::new(viewer));
                            }
                            Err(e) => println!("open: {}", e),
                        }
                    }
                    "mp4" | "mov" => {
                        let bytes = match fs_call(FsRequest::Read { cwd: cwd.clone(), path: path.clone() }) {
                            FsResponse::Data(bytes) => bytes,
                            FsResponse::Err(e) => { println!("open: err: {:?}", e); return; }
                            _ => { println!("open: unexpected response"); return; }
                        };
                        match crate::ui::video::Mp4Decoder::new(bytes) {
                            Ok(decoder) => {
                                let player = crate::ui::apps::video::VideoPlayer::new(path.clone(), alloc::boxed::Box::new(decoder));
                                crate::ui::window::open_app(alloc::boxed::Box::new(player));
                            }
                            Err(e) => println!("open: {}", e),
                        }
                    }
                    _ => { println!("open: unknown extension: {}", ext); }
                }
            }
        }
    }
}
