//! File descriptors: what a program's numbered handles (0 stdin, 1 stdout,
//! 2 stderr, then whatever it opens) point at.
//!
//! - `Console`: writes print on the console; reads take a line typed there,
//!   but only for a program started with `run` (anyone else reads the end
//!   of input straight away). Ctrl+D on an empty line is the end of input.
//! - `Null`: writes are thrown away, reads are the end of input. A program
//!   started with `bg` has it for 0-2.
//! - `File`: an open file. The filesystem reads and writes whole files, so
//!   opening one reads all of it into memory; reads, writes and seeks work
//!   on that copy, and it's written back once the last descriptor for it
//!   is closed (or its program ends).
//! - a pipe's two ends: bytes written to one come out of the other, in
//!   order. A read waits for bytes, and gets the end of input once every
//!   write end is closed; a write waits for room, and fails once every read
//!   end is.
//!
//! A descriptor is an `Fd`, shared: `dup2` and starting a program with it
//! as its stdin or stdout give out more references to the same thing (and
//! the same position in a file), as on Unix.

use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use crate::filesys::fs_ipc::{fs_call, FsRequest, FsResponse};
use crate::process::Console;
use crate::scheduler::{self, wait::WaitQueue};
use crate::std::lock::irq_mutex::IrqMutex;

pub type Fd = Arc<Object>;

/// How much a pipe holds before writes wait.
pub const PIPE_CAPACITY: usize = 64 * 1024;

// open() flags, as in tcclibc's fcntl.h (Linux's values).
pub const O_ACCMODE: u64 = 3;
pub const O_RDONLY: u64 = 0;
pub const O_WRONLY: u64 = 1;
pub const O_CREAT: u64 = 0o100;
pub const O_EXCL: u64 = 0o200;
pub const O_TRUNC: u64 = 0o1000;
pub const O_APPEND: u64 = 0o2000;

pub enum Object {
    Console,
    Null,
    File(IrqMutex<OpenFile>),
    PipeRead(Arc<Pipe>),
    PipeWrite(Arc<Pipe>),
}

pub struct OpenFile {
    cwd: String,
    path: String,
    data: Vec<u8>,
    pos: usize,
    readable: bool,
    writable: bool,
    append: bool,
    /// Changed since it was read: write it back.
    dirty: bool,
}

pub struct Pipe {
    buf: IrqMutex<VecDeque<u8>>,
    readers: AtomicUsize,
    writers: AtomicUsize,
    /// Woken when bytes go in or come out, or an end closes.
    changed: WaitQueue,
}

/// What's left of the line the foreground program is reading from the
/// console, if it asked for less than a whole line.
static CONSOLE_PENDING: IrqMutex<Vec<u8>> = IrqMutex::new(Vec::new());

fn fs_error(response: FsResponse) -> String {
    match response {
        FsResponse::Err(e) => e.to_string(),
        _ => String::from("unexpected response from the filesystem"),
    }
}

/// Opens `path` (relative to `cwd`) with open()'s `flags`.
pub fn open(cwd: String, path: &str, flags: u64) -> Result<Fd, String> {
    let access = flags & O_ACCMODE;
    let (readable, writable) = (access != O_WRONLY, access != O_RDONLY);
    let existing = match fs_call(FsRequest::Read { cwd: cwd.clone(), path: String::from(path) }) {
        FsResponse::Data(data) => Some(data),
        FsResponse::Err(crate::filesys::backend::FsError::NotFound) => None,
        e => return Err(fs_error(e)),
    };
    if existing.is_some() && flags & O_CREAT != 0 && flags & O_EXCL != 0 {
        return Err(format!("{}: AlreadyExists", path));
    }
    if existing.is_none() && flags & O_CREAT == 0 {
        return Err(format!("{}: NotFound", path));
    }
    let truncate = writable && flags & O_TRUNC != 0;
    let data = match existing {
        Some(data) if !truncate => data,
        _ => {
            // A new or emptied file shows up as one straight away.
            if !writable {
                return Err(format!("{}: can't create a file opened only for reading", path));
            }
            match fs_call(FsRequest::Write { cwd: cwd.clone(), path: String::from(path), data: Vec::new() }) {
                FsResponse::Ok => Vec::new(),
                e => return Err(fs_error(e)),
            }
        }
    };
    Ok(Arc::new(Object::File(IrqMutex::new(OpenFile {
        cwd,
        path: String::from(path),
        data,
        pos: 0,
        readable,
        writable,
        append: flags & O_APPEND != 0,
        dirty: false,
    }))))
}

/// A new pipe: (read end, write end).
pub fn pipe() -> (Fd, Fd) {
    let pipe = Arc::new(Pipe {
        buf: IrqMutex::new(VecDeque::new()),
        readers: AtomicUsize::new(1),
        writers: AtomicUsize::new(1),
        changed: WaitQueue::new(),
    });
    (Arc::new(Object::PipeRead(pipe.clone())), Arc::new(Object::PipeWrite(pipe)))
}

pub fn console() -> Fd {
    Arc::new(Object::Console)
}

pub fn null() -> Fd {
    Arc::new(Object::Null)
}

impl Object {
    /// Reads into `buf`: Some(0) at the end of input, None if `wait` is
    /// false and nothing has come in yet.
    pub fn read(&self, buf: &mut [u8], wait: bool) -> Result<Option<usize>, String> {
        if buf.is_empty() {
            return Ok(Some(0));
        }
        match self {
            Object::Null => Ok(Some(0)),
            Object::Console => {
                if crate::process::console() != Some(Console::Foreground) {
                    return Ok(Some(0));
                }
                if CONSOLE_PENDING.lock().is_empty() {
                    if !wait {
                        return Ok(None);
                    }
                    // None: killed while waiting (Ctrl+C), which ends it.
                    crate::drivers::uart::take_end_of_input();
                    let Some(line) = crate::drivers::uart::Uart {}.read_s_unless(scheduler::current_killed) else { return Ok(Some(0)) };
                    if crate::drivers::uart::take_end_of_input() {
                        return Ok(Some(0));
                    }
                    let mut pending = CONSOLE_PENDING.lock();
                    pending.extend_from_slice(line.as_bytes());
                    pending.push(b'\n');
                }
                let mut pending = CONSOLE_PENDING.lock();
                let n = pending.len().min(buf.len());
                buf[..n].copy_from_slice(&pending[..n]);
                pending.drain(..n);
                Ok(Some(n))
            }
            Object::File(file) => {
                let mut f = file.lock();
                if !f.readable {
                    return Err(String::from("not open for reading"));
                }
                let start = f.pos.min(f.data.len());
                let n = (f.data.len() - start).min(buf.len());
                buf[..n].copy_from_slice(&f.data[start..start + n]);
                f.pos = start + n;
                Ok(Some(n))
            }
            Object::PipeRead(pipe) => {
                let mut n = 0;
                let has_data = || !pipe.buf.lock().is_empty() || pipe.writers.load(Ordering::Acquire) == 0 || scheduler::current_killed();
                if wait {
                    pipe.changed.wait_until(has_data);
                }
                {
                    let mut b = pipe.buf.lock();
                    while n < buf.len() {
                        let Some(byte) = b.pop_front() else { break };
                        buf[n] = byte;
                        n += 1;
                    }
                }
                if n > 0 {
                    pipe.changed.wake_all();
                    return Ok(Some(n));
                }
                if pipe.writers.load(Ordering::Acquire) == 0 || wait {
                    return Ok(Some(0));
                }
                Ok(None)
            }
            Object::PipeWrite(_) => Err(String::from("that's a pipe's write end")),
        }
    }

    /// Writes `data`: how much went, or None if `wait` is false and none
    /// of it could go yet. A waiting write only returns early if its
    /// program is killed.
    pub fn write(&self, data: &[u8], wait: bool) -> Result<Option<usize>, String> {
        match self {
            Object::Null => Ok(Some(data.len())),
            Object::Console => {
                crate::drivers::uart::console_write(data);
                Ok(Some(data.len()))
            }
            Object::File(file) => {
                let mut f = file.lock();
                if !f.writable {
                    return Err(String::from("not open for writing"));
                }
                if f.append {
                    f.pos = f.data.len();
                }
                let pos = f.pos;
                let end = pos + data.len();
                if end > f.data.len() {
                    f.data.resize(end, 0);
                }
                f.data[pos..end].copy_from_slice(data);
                f.pos = end;
                f.dirty = true;
                Ok(Some(data.len()))
            }
            Object::PipeWrite(pipe) => {
                let mut done = 0;
                loop {
                    if pipe.readers.load(Ordering::Acquire) == 0 {
                        return if done > 0 { Ok(Some(done)) } else { Err(String::from("nothing reads from that pipe any more")) };
                    }
                    {
                        let mut b = pipe.buf.lock();
                        let n = (PIPE_CAPACITY - b.len().min(PIPE_CAPACITY)).min(data.len() - done);
                        b.extend(&data[done..done + n]);
                        done += n;
                    }
                    pipe.changed.wake_all();
                    if done == data.len() || !wait || scheduler::current_killed() {
                        return Ok(if done == 0 && !wait { None } else { Some(done) });
                    }
                    pipe.changed.wait_until(|| {
                        pipe.buf.lock().len() < PIPE_CAPACITY || pipe.readers.load(Ordering::Acquire) == 0 || scheduler::current_killed()
                    });
                }
            }
            Object::PipeRead(_) => Err(String::from("that's a pipe's read end")),
        }
    }

    /// Moves a file's position: `whence` 0 from the start, 1 from where it
    /// is, 2 from the end. Returns the new position.
    pub fn seek(&self, offset: i64, whence: u64) -> Result<u64, String> {
        let Object::File(file) = self else { return Err(String::from("only files can seek")) };
        let mut f = file.lock();
        let base = match whence {
            0 => 0,
            1 => f.pos as i64,
            2 => f.data.len() as i64,
            _ => return Err(String::from("bad whence")),
        };
        let pos = base.checked_add(offset).filter(|&p| p >= 0).ok_or_else(|| String::from("bad offset"))?;
        f.pos = pos as usize;
        Ok(pos as u64)
    }

    pub fn is_console(&self) -> bool {
        matches!(self, Object::Console)
    }

    /// Writes a changed file back to the disk now.
    pub fn flush(&self) -> Result<(), String> {
        let Object::File(file) = self else { return Ok(()) };
        let (cwd, path, data) = {
            let mut f = file.lock();
            if !f.dirty {
                return Ok(());
            }
            f.dirty = false;
            (f.cwd.clone(), f.path.clone(), f.data.clone())
        };
        match fs_call(FsRequest::Write { cwd, path: path.clone(), data }) {
            FsResponse::Ok => Ok(()),
            e => Err(fs_error(e)),
        }
    }
}

impl Drop for Object {
    fn drop(&mut self) {
        match self {
            Object::File(_) => if let Err(e) = self.flush() {
                log::warn!("writing back {}", e);
            },
            Object::PipeRead(pipe) => {
                pipe.readers.fetch_sub(1, Ordering::AcqRel);
                pipe.changed.wake_all();
            }
            Object::PipeWrite(pipe) => {
                pipe.writers.fetch_sub(1, Ordering::AcqRel);
                pipe.changed.wake_all();
            }
            Object::Console | Object::Null => {}
        }
    }
}

#[cfg(feature = "test")]
pub mod tests {
    use super::*;

    fn test_pipe_passes_bytes_in_order() {
        let (r, w) = pipe();
        assert_eq!(w.write(b"hello ", true), Ok(Some(6)));
        assert_eq!(w.write(b"world", true), Ok(Some(5)));
        let mut buf = [0u8; 4];
        assert_eq!(r.read(&mut buf, true), Ok(Some(4)));
        assert_eq!(&buf, b"hell");
        let mut rest = [0u8; 32];
        assert_eq!(r.read(&mut rest, true), Ok(Some(7)));
        assert_eq!(&rest[..7], b"o world");
    }

    fn test_pipe_read_without_waiting() {
        let (r, w) = pipe();
        let mut buf = [0u8; 8];
        assert_eq!(r.read(&mut buf, false), Ok(None), "nothing written yet");
        w.write(b"x", true).unwrap();
        assert_eq!(r.read(&mut buf, false), Ok(Some(1)));
        drop(w);
        assert_eq!(r.read(&mut buf, false), Ok(Some(0)), "end of input once the write end is closed");
    }

    fn test_pipe_end_of_input_after_buffered_bytes() {
        let (r, w) = pipe();
        w.write(b"last", true).unwrap();
        drop(w);
        let mut buf = [0u8; 8];
        assert_eq!(r.read(&mut buf, true), Ok(Some(4)));
        assert_eq!(r.read(&mut buf, true), Ok(Some(0)));
    }

    fn test_pipe_write_fails_without_readers() {
        let (r, w) = pipe();
        drop(r);
        assert!(w.write(b"lost", true).is_err());
    }

    fn test_pipe_full_write_without_waiting() {
        let (r, w) = pipe();
        let big = alloc::vec![7u8; PIPE_CAPACITY + 100];
        assert_eq!(w.write(&big, false), Ok(Some(PIPE_CAPACITY)), "only what fits goes");
        assert_eq!(w.write(b"more", false), Ok(None));
        let mut buf = alloc::vec![0u8; 1000];
        assert_eq!(r.read(&mut buf, true), Ok(Some(1000)));
        assert_eq!(w.write(b"more", false), Ok(Some(4)));
    }

    fn test_shared_pipe_end_closes_with_its_last_reference() {
        let (r, w) = pipe();
        let w2 = w.clone();
        drop(w);
        let mut buf = [0u8; 8];
        assert_eq!(r.read(&mut buf, false), Ok(None), "a copy of the write end is still open");
        drop(w2);
        assert_eq!(r.read(&mut buf, false), Ok(Some(0)));
    }

    fn test_null_reads_nothing_and_takes_everything() {
        let n = null();
        let mut buf = [0u8; 8];
        assert_eq!(n.read(&mut buf, true), Ok(Some(0)));
        assert_eq!(n.write(b"gone", true), Ok(Some(4)));
    }

    pub fn run() {
        crate::test::run_all(&[
            &test_pipe_passes_bytes_in_order,
            &test_pipe_read_without_waiting,
            &test_pipe_end_of_input_after_buffered_bytes,
            &test_pipe_write_fails_without_readers,
            &test_pipe_full_write_without_waiting,
            &test_shared_pipe_end_closes_with_its_last_reference,
            &test_null_reads_nothing_and_takes_everything,
        ]);
    }
}
