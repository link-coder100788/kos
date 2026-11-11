use crate::scheduler::{Request, task::TrapFrame};
use crate::{drivers::uart::Uart, filesys::fs_ipc::{FsRequest, fs_call}, mm::allocator::ALLOCATOR, println, scheduler, sections::{__syscalls_end, __syscalls_start}, shell::fs::current_cwd, ui::window::{WindowElementDesc, WindowId}};
use crate::process::Console;
use kos_derive::syscall;
use crate::filesys::fs_ipc::FsResponse;
use alloc::format;
use alloc::string::ToString;
use alloc::vec::Vec;

pub type SyscallFn = fn(&mut SyscallContext);

#[repr(C)]
pub struct SyscallEntry {
    pub number: u64,
    pub name: &'static str,
    pub handler: SyscallFn,
}

/// Syscall handlers read their arguments from, and write results into,
/// the caller's saved registers.
pub type SyscallContext = TrapFrame;

/// `svc #9`: give up the rest of the time slice.
pub const SYS_YIELD: u64 = 9;

/// The longest NUL-terminated string a program can pass (titles, labels).
const MAX_CSTR: usize = 4096;

// ---- program memory ------------------------------------------------------------
// Programs pass pointers into their own address space, which is what TTBR0
// holds during their syscalls. Every range is checked against what programs
// have mapped first, so a bad pointer gets an error back instead of making
// the kernel fault (or read kernel memory). Kernel tasks' pointers are
// trusted.

fn user_ok(frame: &SyscallContext, ptr: u64, len: usize, align: usize) -> bool {
    !frame.is_user() || (ptr as usize % align == 0 && crate::process::user_range_ok(ptr as usize, len))
}

fn user_bytes<'a>(frame: &SyscallContext, ptr: u64, len: u64) -> Option<&'a [u8]> {
    if len == 0 {
        return Some(&[]);
    }
    let len = usize::try_from(len).ok()?;
    user_ok(frame, ptr, len, 1).then(|| unsafe { core::slice::from_raw_parts(ptr as *const u8, len) })
}

fn user_bytes_mut<'a>(frame: &SyscallContext, ptr: u64, len: u64) -> Option<&'a mut [u8]> {
    if len == 0 {
        return Some(&mut []);
    }
    let len = usize::try_from(len).ok()?;
    user_ok(frame, ptr, len, 1).then(|| unsafe { core::slice::from_raw_parts_mut(ptr as *mut u8, len) })
}

fn user_str<'a>(frame: &SyscallContext, ptr: u64, len: u64) -> Option<&'a str> {
    core::str::from_utf8(user_bytes(frame, ptr, len)?).ok()
}

/// A NUL-terminated UTF-8 string, without the NUL.
fn user_cstr<'a>(frame: &SyscallContext, ptr: u64) -> Option<&'a str> {
    let mut len = 0;
    loop {
        if len == MAX_CSTR || !user_ok(frame, ptr.checked_add(len as u64)?, 1, 1) {
            return None;
        }
        if unsafe { *((ptr as usize + len) as *const u8) } == 0 {
            break;
        }
        len += 1;
    }
    user_str(frame, ptr, len as u64)
}

/// Copies as much of `data` as fits into the caller's buffer at `ptr`
/// (`max` bytes; 0 just asks the size), and returns `data`'s full length,
/// or u64::MAX if the buffer isn't the caller's.
fn copy_out(frame: &SyscallContext, ptr: u64, max: u64, data: &[u8]) -> u64 {
    let Some(buf) = user_bytes_mut(frame, ptr, max) else { return u64::MAX };
    let n = data.len().min(buf.len());
    buf[..n].copy_from_slice(&data[..n]);
    data.len() as u64
}

/// Records why a call failed, for `last_error`, and returns -1.
fn fail(why: &str) -> u64 {
    crate::process::set_error(why);
    u64::MAX
}

// ---- console -------------------------------------------------------------------

/// Whether the caller's output should reach the console: not for programs
/// started with `bg`.
fn may_print() -> bool {
    crate::process::console() != Some(Console::Detached)
}

/// `exit(status)`. The program's files are closed (and written back)
/// before it ends.
#[syscall(0)]
pub fn syscall_exit(frame: &mut SyscallContext) {
    crate::process::set_exit_status(frame[0] as i32);
    drop(crate::process::fd_take_all());
    frame.request = Request::Exit as u64;
}

/// Writes to the caller's stdout; a kernel task's go to the console.
fn write_stdout(bytes: &[u8]) {
    match crate::process::fd_get(1) {
        Some(out) => { let _ = out.write(bytes, true); }
        None if crate::process::console().is_none() => crate::drivers::uart::console_write(bytes),
        None => {}
    }
}

#[syscall(1)]
pub fn syscall_write_byte(frame: &mut SyscallContext) {
    write_stdout(&[frame.regs[0] as u8]);
}

#[syscall(2)]
pub fn syscall_write_bytes(frame: &mut SyscallContext) {
    // Returns nothing: the libc doesn't expect x0 to change.
    if let Some(bytes) = user_bytes(frame, frame[0], frame[1]) {
        write_stdout(bytes);
    }
}

/// Reads a line from stdin, without its newline: its length, or u64::MAX
/// at the end of the input (a pipe or file's, or Ctrl+D on the console).
/// A line longer than the buffer is cut short.
#[syscall(3)]
pub fn syscall_read_line(frame: &mut SyscallContext) {
    let Some(buf) = user_bytes_mut(frame, frame[0], frame[1]) else { frame[0] = u64::MAX; return };
    let Some(stdin) = crate::process::fd_get(0) else { frame[0] = u64::MAX; return };
    if stdin.is_console() {
        // Only a program started with `run` has the console: the shell is
        // waiting for it to finish, so it can read the console directly.
        // Anyone else gets no input.
        if crate::process::console() != Some(Console::Foreground) {
            frame[0] = 0;
            return;
        }
        crate::drivers::uart::take_end_of_input();
        let line = Uart {}.read_s_unless(scheduler::current_killed).unwrap_or_default();
        if crate::drivers::uart::take_end_of_input() {
            frame[0] = u64::MAX;
            return;
        }
        let len = line.len().min(buf.len());
        buf[..len].copy_from_slice(&line.as_bytes()[..len]);
        frame[0] = len as u64;
        return;
    }
    let (mut len, mut any) = (0, false);
    loop {
        let mut byte = [0u8];
        match stdin.read(&mut byte, true) {
            Ok(Some(1)) => {}
            _ => break,
        }
        any = true;
        if byte[0] == b'\n' {
            break;
        }
        if len < buf.len() {
            buf[len] = byte[0];
            len += 1;
        }
    }
    if len > 0 && buf[len - 1] == b'\r' {
        len -= 1;
    }
    frame[0] = if any { len as u64 } else { u64::MAX };
}

// ---- memory --------------------------------------------------------------------

mod memory {
    use kos_derive::const_unique;

    pub const ALLOC: u64 = 0;     // alloc(size) -> ptr
    pub const ALLOC_N: u64 = 1;   // alloc(size, n) -> alloc(size * n) -> ptr
    pub const ALLOC_Z: u64 = 2;   // alloc(size) -> ptr, zeroed
    pub const ALLOC_N_Z: u64 = 3; // alloc(size, n) -> alloc(size * n) -> ptr, zeroed
    pub const MEM_FREE: u64 = 4;  // the program's free heap
    pub const MEM_TOTAL: u64 = 5; // the program's total heap
    pub const DEALLOC: u64 = 6;   // free(ptr) -> 0, or u64::MAX if ptr isn't the program's
    pub const MEM_SIZE: u64 = 7;  // size(ptr) -> how big the allocation is, or 0

    const_unique!(ALLOC, ALLOC_N, ALLOC_Z, ALLOC_N_Z, MEM_FREE, MEM_TOTAL, DEALLOC, MEM_SIZE);
}

/// Programs get heap memory inside their own address space, from
/// `process`; it all goes away with the program. For kernel callers, the
/// sizes are the kernel heap's.
#[syscall(4)]
pub fn syscall_memory(frame: &mut SyscallContext) {
    let syscall_num = frame.regs[16];
    let size = match syscall_num {
        memory::ALLOC | memory::ALLOC_Z => Some(frame.regs[0] as usize),
        memory::ALLOC_N | memory::ALLOC_N_Z => (frame.regs[0] as usize).checked_mul(frame.regs[1] as usize),
        _ => None,
    };
    match syscall_num {
        memory::ALLOC | memory::ALLOC_N | memory::ALLOC_Z | memory::ALLOC_N_Z => {
            let Some(ptr) = size.and_then(crate::process::alloc) else { frame.regs[0] = 0; return; };
            // Freed memory is handed out again, so zero it if asked.
            if matches!(syscall_num, memory::ALLOC_Z | memory::ALLOC_N_Z) {
                unsafe { core::ptr::write_bytes(ptr as *mut u8, 0, size.unwrap()) };
            }
            frame.regs[0] = ptr as u64;
        },
        memory::MEM_FREE | memory::MEM_TOTAL => {
            let (free, total) = crate::process::heap_stats().unwrap_or_else(|| {
                let (_, free, total) = ALLOCATOR.heap_remaining();
                (free as usize, total as usize)
            });
            frame.regs[0] = if syscall_num == memory::MEM_FREE { free } else { total } as u64;
        },
        memory::DEALLOC => {
            let ptr = frame.regs[0] as usize;
            frame.regs[0] = if ptr == 0 || crate::process::free(ptr) { 0 } else { u64::MAX };
        },
        memory::MEM_SIZE => {
            frame.regs[0] = crate::process::alloc_size(frame.regs[0] as usize).unwrap_or(0) as u64;
        },
        _ => { log::warn!("memory syscall: unknown sub-command {}", frame[16]); }
    }
}

// ---- files ---------------------------------------------------------------------

mod filesystem {
    use kos_derive::const_unique;

    pub const FS_WRITE: u64 = 0;  // (path, len, data, len) -> 0
    pub const FS_READ: u64 = 1;   // (path, len, buf, max) -> file size
    pub const FS_LIST: u64 = 2;   // (path, len, buf, max) -> listing size
    pub const FS_REMOVE: u64 = 3; // (path, len) -> 0
    pub const FS_MKDIR: u64 = 4;  // (path, len) -> 0
    pub const FS_CWD: u64 = 5;    // (buf, max) -> the current directory's length

    const_unique!(FS_WRITE, FS_READ, FS_LIST, FS_REMOVE, FS_MKDIR, FS_CWD);
}

/// Files, with paths relative to the shell's current directory. Errors
/// return u64::MAX, with the reason kept for `last_error`.
#[syscall(5)]
pub fn syscall_filesystem(frame: &mut SyscallContext) {
    let syscall_num = frame.regs[16];
    if syscall_num == filesystem::FS_CWD {
        frame[0] = copy_out(frame, frame[0], frame[1], current_cwd().as_bytes());
        return;
    }
    let Some(path) = user_str(frame, frame[0], frame[1]) else { frame[0] = fail("bad path"); return };
    let cwd = current_cwd();
    let path = path.to_string();
    let failed = |op: &str, e: FsResponse| match e {
        FsResponse::Err(e) => { log::debug!("{} {}: {:?}", op, path, e); fail(&e.to_string()) }
        _ => { log::warn!("{} {}: unexpected response", op, path); fail("unexpected response from the filesystem") }
    };
    frame[0] = match syscall_num {
        filesystem::FS_WRITE => {
            let Some(data) = user_bytes(frame, frame[2], frame[3]) else { frame[0] = fail("bad buffer"); return };
            match fs_call(FsRequest::Write { cwd, path: path.clone(), data: data.to_vec() }) {
                FsResponse::Ok => 0,
                e => failed("fs_write", e),
            }
        }
        // Copy at most what fits; return the full size so the caller can
        // tell if it was truncated.
        filesystem::FS_READ => match fs_call(FsRequest::Read { cwd, path: path.clone() }) {
            FsResponse::Data(d) => copy_out(frame, frame[2], frame[3], &d),
            e => failed("fs_read", e),
        },
        // One line per entry: "d\t0\tname" or "f\t<size>\tname".
        filesystem::FS_LIST => match fs_call(FsRequest::List { cwd, path: path.clone() }) {
            FsResponse::Listing(entries) => {
                let mut text = alloc::string::String::new();
                for e in entries.iter().filter(|e| e.name != "." && e.name != "..") {
                    text.push_str(&format!("{}\t{}\t{}\n", if e.is_dir { 'd' } else { 'f' }, e.size, e.name));
                }
                copy_out(frame, frame[2], frame[3], text.as_bytes())
            }
            e => failed("fs_list", e),
        },
        filesystem::FS_REMOVE => match fs_call(FsRequest::Remove { cwd, path: path.clone() }) {
            FsResponse::Ok => 0,
            e => failed("fs_remove", e),
        },
        filesystem::FS_MKDIR => match fs_call(FsRequest::Mkdir { cwd, path: path.clone() }) {
            FsResponse::Ok => 0,
            e => failed("fs_mkdir", e),
        },
        _ => { log::warn!("filesystem syscall: unknown sub-command {}", syscall_num); u64::MAX }
    };
}

// ---- file descriptors -------------------------------------------------------------

mod fds {
    use kos_derive::const_unique;

    pub const OPEN: u64 = 0;   // (path, len, flags) -> fd
    pub const READ: u64 = 1;   // (fd, buf, max, dont_wait) -> bytes read; 0 = end of input; u64::MAX - 1 = nothing yet
    pub const WRITE: u64 = 2;  // (fd, data, len, dont_wait) -> bytes written; u64::MAX - 1 = no room yet
    pub const CLOSE: u64 = 3;  // (fd) -> 0
    pub const SEEK: u64 = 4;   // (fd, offset, whence) -> the new position
    pub const DUP2: u64 = 5;   // (fd, to) -> to
    pub const DUP: u64 = 6;    // (fd) -> a new fd for the same thing
    pub const PIPE: u64 = 7;   // (int[2]) -> 0; [0] reads what [1] writes
    pub const ISATTY: u64 = 8; // (fd) -> 1 if it's the console, else 0

    const_unique!(OPEN, READ, WRITE, CLOSE, SEEK, DUP2, DUP, PIPE, ISATTY);
}

/// File descriptors (see `fd`): files, pipes and the console. Only
/// programs have them. Errors return u64::MAX, with the reason kept for
/// `last_error`.
#[syscall(17)]
pub fn syscall_fd(frame: &mut SyscallContext) {
    use crate::process::{fd_add, fd_get, fd_set, fd_take};
    let syscall_num = frame[16];
    let get = |fd: u64| fd_get(fd).ok_or_else(|| alloc::string::String::from("bad file descriptor"));
    let result: Result<u64, alloc::string::String> = match syscall_num {
        fds::OPEN => match user_str(frame, frame[0], frame[1]) {
            Some(path) => crate::fd::open(current_cwd(), path, frame[2])
                .and_then(|f| fd_add(f).map_err(|_| "too many open files".to_string())),
            None => Err("bad path".to_string()),
        },
        fds::READ => match (get(frame[0]), user_bytes_mut(frame, frame[1], frame[2])) {
            (Ok(f), Some(buf)) => f.read(buf, frame[3] == 0).map(|n| n.map_or(u64::MAX - 1, |n| n as u64)),
            (Err(e), _) => Err(e),
            (_, None) => Err("bad buffer".to_string()),
        },
        fds::WRITE => match (get(frame[0]), user_bytes(frame, frame[1], frame[2])) {
            (Ok(f), Some(data)) => f.write(data, frame[3] == 0).map(|n| n.map_or(u64::MAX - 1, |n| n as u64)),
            (Err(e), _) => Err(e),
            (_, None) => Err("bad buffer".to_string()),
        },
        fds::CLOSE => match fd_take(frame[0]) {
            // The last descriptor for a file writes it back, and says if
            // that failed.
            Some(f) => match alloc::sync::Arc::try_unwrap(f) {
                Ok(object) => object.flush().map(|()| 0),
                Err(_) => Ok(0),
            },
            None => Err("bad file descriptor".to_string()),
        },
        fds::SEEK => get(frame[0]).and_then(|f| f.seek(frame[1] as i64, frame[2])),
        fds::DUP2 => get(frame[0]).and_then(|f| match fd_set(frame[1], f) {
            Ok(old) => { drop(old); Ok(frame[1]) }
            Err(_) => Err("bad file descriptor".to_string()),
        }),
        fds::DUP => get(frame[0]).and_then(|f| fd_add(f).map_err(|_| "too many open files".to_string())),
        fds::PIPE => {
            if !user_ok(frame, frame[0], 8, 4) || !frame.is_user() {
                Err("bad buffer".to_string())
            } else {
                let (r, w) = crate::fd::pipe();
                match fd_add(r) {
                    Err(_) => Err("too many open files".to_string()),
                    Ok(rfd) => match fd_add(w) {
                        Err(_) => { drop(fd_take(rfd)); Err("too many open files".to_string()) }
                        Ok(wfd) => {
                            unsafe { (frame[0] as *mut [i32; 2]).write([rfd as i32, wfd as i32]) };
                            Ok(0)
                        }
                    },
                }
            }
        }
        fds::ISATTY => Ok(fd_get(frame[0]).is_some_and(|f| f.is_console()) as u64),
        _ => Err(format!("unknown file descriptor call {}", syscall_num)),
    };
    frame[0] = result.unwrap_or_else(|e| fail(&e));
}

// ---- windows -------------------------------------------------------------------

mod ui {
    use kos_derive::const_unique;

    pub const UI_CREATE_WINDOW: u64 = 0;
    pub const UI_CLOSE_WINDOW: u64 = 1;
    pub const UI_TEST_WINDOW: u64 = 2;
    pub const UI_POLL_EVENT: u64 = 3;  // poll(id) -> 0 none, n+1 button n clicked, u64::MAX closed
    // Canvases: windows a program draws pixels into (ui/apps/canvas.rs).
    pub const UI_CANVAS_OPEN: u64 = 4;     // (title, width, height) -> id, or u64::MAX
    pub const UI_CANVAS_PRESENT: u64 = 5;  // (id, pixels, count) -> 0, or u64::MAX
    pub const UI_CANVAS_POLL: u64 = 6;     // (id, *event) -> 1 event, 0 none, u64::MAX no such canvas
    pub const UI_CANVAS_CLOSE: u64 = 7;    // (id)

    const_unique!(UI_CREATE_WINDOW, UI_CLOSE_WINDOW, UI_TEST_WINDOW, UI_POLL_EVENT,
        UI_CANVAS_OPEN, UI_CANVAS_PRESENT, UI_CANVAS_POLL, UI_CANVAS_CLOSE);
}

// typedef struct {
//     size_t x, y, w, h;
//     const char* text;
// } ui_element_desc_t;
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RawElementDesc {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
    pub text: u64,
}

impl RawElementDesc {
    /// A label if `h` is 0 (with `w` packing its color and scale), else a
    /// button. None if its text isn't a valid string.
    fn read(&self, frame: &SyscallContext) -> Option<WindowElementDesc> {
        let text = user_cstr(frame, self.text)?.to_string();
        Some(if self.h == 0 {
            WindowElementDesc::Label { x: self.x, y: self.y, text, color: crate::ui::color::rgb_to_xrgb8888((self.w >> 0) as u8, (self.w >> 8) as u8, (self.w >> 16) as u8), scale: self.w >> 24 }
        } else {
            WindowElementDesc::Button { x: self.x, y: self.y, w: self.w, h: self.h, label: text }
        })
    }
}

/// `canvas_event_t` in userspace/libc/canvas.h.
#[repr(C)]
struct RawCanvasEvent {
    kind: i32,
    x: i32,
    y: i32,
    /// The button pressed or released, or every button held during a move.
    buttons: i32,
    key: i32,
}

impl From<crate::ui::apps::canvas::CanvasEvent> for RawCanvasEvent {
    fn from(e: crate::ui::apps::canvas::CanvasEvent) -> Self {
        use crate::ui::apps::canvas::CanvasEvent::*;
        let (kind, x, y, buttons, key) = match e {
            MouseDown { x, y, button } => (1, x, y, button as i32, 0),
            MouseUp { x, y, button } => (2, x, y, button as i32, 0),
            MouseMove { x, y, buttons } => (3, x, y, buttons as i32, 0),
            Key(code) => (4, 0, 0, 0, code as i32),
            Closed => (5, 0, 0, 0, 0),
            Scroll(clicks) => (6, 0, 0, 0, clicks),
        };
        RawCanvasEvent { kind, x, y, buttons, key }
    }
}

/// Window syscalls. A program can only use windows it opened itself, and
/// they're closed for it when it ends.
#[syscall(6)]
pub fn syscall_ui(frame: &mut SyscallContext) {
    use crate::process::{self, WindowKind};
    let syscall_num = frame.regs[16];
    match syscall_num {
        ui::UI_CREATE_WINDOW => {
            let count = frame[2] as usize;
            let size = count.checked_mul(size_of::<RawElementDesc>());
            let elements = user_cstr(frame, frame[0]).zip(size.filter(|&n| user_ok(frame, frame[1], n, 8))).and_then(|(title, _)| {
                let raw = unsafe { core::slice::from_raw_parts(frame[1] as *const RawElementDesc, count) };
                Some((title, raw.iter().map(|e| e.read(frame)).collect::<Option<Vec<_>>>()?))
            });
            frame[0] = match elements {
                Some((title, elements)) => {
                    let id = crate::ui::window::open_window(title, elements);
                    process::add_window(id, WindowKind::Widgets);
                    id as u64
                }
                None => u64::MAX,
            };
        }
        ui::UI_CLOSE_WINDOW => {
            let id = frame[0] as WindowId;
            if process::owns_window(id, WindowKind::Widgets) {
                process::remove_window(id);
                crate::ui::window::close_window(id);
            }
        }
        ui::UI_TEST_WINDOW => {
            use alloc::vec;
            use alloc::string::String;
            let elements = vec![
                WindowElementDesc::Label {
                    x: 10,
                    y: 10,
                    text: String::from("SYSCALL TEST WINDOW"),
                    color: crate::ui::color::rgb_to_xrgb8888(0, 0, 0),
                    scale: 3,
                },
                WindowElementDesc::Button {
                    x: 10,
                    y: 50,
                    w: 120,
                    h: 30,
                    label: String::from("BUTTON A"),
                },
                WindowElementDesc::Button {
                    x: 140,
                    y: 50,
                    w: 120,
                    h: 30,
                    label: String::from("BUTTON B"),
                },
            ];
            let id = crate::ui::window::open_window("Syscall Test", elements);
            process::add_window(id, WindowKind::Widgets);
            frame[0] = id as u64;
        }
        ui::UI_POLL_EVENT => {
            use crate::ui::window::WindowEvent;
            let id = frame[0] as WindowId;
            frame[0] = if !process::owns_window(id, WindowKind::Widgets) {
                u64::MAX
            } else {
                match crate::ui::window::poll_window_event(id) {
                    None => 0,
                    Some(WindowEvent::ElementActivated(n)) => n as u64 + 1,
                    Some(WindowEvent::Closed) => u64::MAX,
                }
            };
        }
        ui::UI_CANVAS_OPEN => {
            let title = user_cstr(frame, frame[0]).unwrap_or("Canvas");
            frame[0] = match crate::ui::apps::canvas::open(title, frame[1] as usize, frame[2] as usize) {
                Some(id) => {
                    process::add_window(id, WindowKind::Canvas);
                    id as u64
                }
                None => u64::MAX,
            };
        }
        ui::UI_CANVAS_PRESENT => {
            use crate::ui::apps::canvas;
            let id = frame[0] as WindowId;
            // The length must match the canvas, so a wrong size can't
            // read past the end of the program's frame.
            let ok = match canvas::pixel_count(id) {
                Some(n) if process::owns_window(id, WindowKind::Canvas) && frame[1] != 0 && frame[2] as usize == n
                    && user_ok(frame, frame[1], n * 4, 4) => {
                    let pixels = unsafe { core::slice::from_raw_parts(frame[1] as *const u32, n) };
                    canvas::present(id, pixels)
                }
                _ => false,
            };
            frame[0] = if ok { 0 } else { u64::MAX };
        }
        ui::UI_CANVAS_POLL => {
            let id = frame[0] as WindowId;
            let out = frame[1];
            if !process::owns_window(id, WindowKind::Canvas) || (out != 0 && !user_ok(frame, out, size_of::<RawCanvasEvent>(), 4)) {
                frame[0] = u64::MAX;
                return;
            }
            frame[0] = match crate::ui::apps::canvas::poll(id) {
                Ok(Some(event)) => {
                    if out != 0 {
                        unsafe { (out as *mut RawCanvasEvent).write(event.into()) };
                    }
                    1
                }
                Ok(None) => 0,
                Err(()) => u64::MAX,
            };
        }
        ui::UI_CANVAS_CLOSE => {
            let id = frame[0] as WindowId;
            if process::owns_window(id, WindowKind::Canvas) {
                process::remove_window(id);
                crate::ui::apps::canvas::close(id);
            }
        }
        _ => { log::warn!("ui syscall: unknown sub-command {}", frame[16]); }
    }
}

// ---- time ----------------------------------------------------------------------

mod time {
    use kos_derive::const_unique;

    pub const TIME_DELAY_MS: u64 = 0;
    pub const TIME_DELAY_S: u64 = 1;
    pub const TIME_UPTIME_MS: u64 = 2;  // -> milliseconds since boot
    pub const TIME_UNIX: u64 = 3;       // -> seconds since 1970 (UTC), or u64::MAX without a clock
    pub const TIME_UTC_OFFSET: u64 = 4; // -> the local time zone's offset from UTC, in seconds

    const_unique!(TIME_DELAY_MS, TIME_DELAY_S, TIME_UPTIME_MS, TIME_UNIX, TIME_UTC_OFFSET);
}

#[syscall(7)]
pub fn syscall_time(frame: &mut SyscallContext) {
    let syscall_number = frame[16];
    match syscall_number {
        time::TIME_DELAY_MS => {
            let ms = frame[0];
            crate::arch::timer::delay_ms(ms);
        }
        time::TIME_DELAY_S => {
            let s = frame[0];
            crate::arch::timer::delay_ms(s.saturating_mul(1000));
        }
        time::TIME_UPTIME_MS => frame[0] = crate::arch::timer::uptime_ms(),
        time::TIME_UNIX => frame[0] = crate::drivers::rtc::unix_time().unwrap_or(u64::MAX),
        time::TIME_UTC_OFFSET => frame[0] = crate::drivers::rtc::utc_offset() as i64 as u64,
        _ => { log::warn!("time syscall: unknown sub-command {}", frame[16]); }
    }
}

/// `args(buf, max_len)`: copies up to `max_len` bytes of the program's
/// command-line arguments (everything after its name) into `buf`, and
/// returns their full length.
#[syscall(10)]
pub fn syscall_args(frame: &mut SyscallContext) {
    let args = crate::process::args().unwrap_or_default();
    let Some(buf) = user_bytes_mut(frame, frame[0], frame[1]) else { frame[0] = u64::MAX; return };
    let n = args.len().min(buf.len());
    buf[..n].copy_from_slice(&args.as_bytes()[..n]);
    frame[0] = args.len() as u64;
}

// ---- random numbers ------------------------------------------------------------

/// `random(buf, len)`: fills `buf` with random bytes (rand::fill). Returns
/// 0, or u64::MAX if this CPU has no random number generator to seed it.
#[syscall(11)]
pub fn syscall_random(frame: &mut SyscallContext) {
    if !crate::rand::available() {
        frame[0] = fail("this CPU has no random number generator");
        return;
    }
    let Some(buf) = user_bytes_mut(frame, frame[0], frame[1]) else { frame[0] = fail("bad buffer"); return };
    crate::rand::fill(buf);
    frame[0] = 0;
}

// ---- crypto --------------------------------------------------------------------

mod crypto {
    use kos_derive::const_unique;

    pub const SHA256: u64 = 0;      // (data, len, out[32])
    pub const SHA512: u64 = 1;      // (data, len, out[64])
    pub const HMAC_SHA256: u64 = 2; // (key, key_len, data, data_len, out[32])
    pub const AES_GCM_SEAL: u64 = 3; // (aead*): encrypts in place, writes the tag
    pub const AES_GCM_OPEN: u64 = 4; // (aead*): checks the tag, decrypts in place
    pub const ARGON2ID_HASH: u64 = 5;   // (password, len, out, max) -> the PHC string's length
    pub const ARGON2ID_VERIFY: u64 = 6; // (password, len, phc) -> 1 if it matches, else 0

    const_unique!(SHA256, SHA512, HMAC_SHA256, AES_GCM_SEAL, AES_GCM_OPEN, ARGON2ID_HASH, ARGON2ID_VERIFY);
}

/// What a program passes for AES-256-GCM (libc's `aead_t`): pointers to
/// the 32-byte key and 12-byte nonce, the additional data and its length,
/// the data and its length, and a pointer to the 16-byte tag.
const AEAD_SIZE: usize = 7 * 8;

/// Copies `bytes` to the program's `ptr`, which must have room for all of
/// them. Returns 0, or fails.
fn write_out(frame: &SyscallContext, ptr: u64, bytes: &[u8]) -> u64 {
    match user_bytes_mut(frame, ptr, bytes.len() as u64) {
        Some(out) => { out.copy_from_slice(bytes); 0 }
        None => fail("bad output buffer"),
    }
}

#[syscall(15)]
pub fn syscall_crypto(frame: &mut SyscallContext) {
    let syscall_num = frame[16];
    frame[0] = match syscall_num {
        crypto::SHA256 | crypto::SHA512 => {
            let Some(data) = user_bytes(frame, frame[0], frame[1]) else { frame[0] = fail("bad data"); return };
            if syscall_num == crypto::SHA256 {
                write_out(frame, frame[2], &crate::crypto::sha256(data))
            } else {
                write_out(frame, frame[2], &crate::crypto::sha512(data))
            }
        }
        crypto::HMAC_SHA256 => {
            let (Some(key), Some(data)) = (user_bytes(frame, frame[0], frame[1]), user_bytes(frame, frame[2], frame[3]))
            else { frame[0] = fail("bad key or data"); return };
            write_out(frame, frame[4], &crate::crypto::hmac_sha256(key, data))
        }
        crypto::AES_GCM_SEAL | crypto::AES_GCM_OPEN => {
            let Some(raw) = user_ok(frame, frame[0], AEAD_SIZE, 8).then(|| unsafe {
                core::slice::from_raw_parts(frame[0] as *const u64, 7)
            }) else { frame[0] = fail("bad aead_t"); return };
            let (key, nonce, aad, aad_len, data, len, tag) = (raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6]);
            let (Some(key), Some(nonce), Some(aad), Some(data), Some(tag)) = (
                user_bytes(frame, key, 32), user_bytes(frame, nonce, 12), user_bytes(frame, aad, aad_len),
                user_bytes_mut(frame, data, len), user_bytes_mut(frame, tag, 16),
            ) else { frame[0] = fail("bad key, nonce, data or tag"); return };
            let (key, nonce) = (key.try_into().unwrap(), nonce.try_into().unwrap());
            if syscall_num == crypto::AES_GCM_SEAL {
                tag.copy_from_slice(&crate::crypto::aes256gcm_seal(key, nonce, aad, data));
                0
            } else if crate::crypto::aes256gcm_open(key, nonce, aad, data, (&*tag).try_into().unwrap()) {
                0
            } else {
                fail("the data or its tag has been changed")
            }
        }
        crypto::ARGON2ID_HASH => {
            let Some(password) = user_bytes(frame, frame[0], frame[1]) else { frame[0] = fail("bad password"); return };
            let phc = crate::crypto::argon::hash(password);
            // With a NUL after it, if there's room.
            let mut out = phc.into_bytes();
            out.push(0);
            match copy_out(frame, frame[2], frame[3], &out) {
                u64::MAX => fail("bad output buffer"),
                n if n > frame[3] => fail("the output buffer is too small"),
                n => n - 1,
            }
        }
        crypto::ARGON2ID_VERIFY => {
            let (Some(password), Some(phc)) = (user_bytes(frame, frame[0], frame[1]), user_cstr(frame, frame[2]))
            else { frame[0] = fail("bad password or hash"); return };
            crate::crypto::argon::verify(password, phc) as u64
        }
        _ => { log::warn!("crypto syscall: unknown sub-command {}", syscall_num); u64::MAX }
    };
}

// ---- network -------------------------------------------------------------------

mod net {
    use kos_derive::const_unique;

    pub const HTTP: u64 = 0;         // (request*) -> status code; keeps the response
    pub const HTTP_BODY: u64 = 1;    // (buf, max) -> the last response's body length
    pub const HTTP_HEADERS: u64 = 2; // (buf, max) -> its "Name: value\n" headers' length
    pub const PING: u64 = 3;         // (host) -> round trip in ms
    pub const ADDRESS: u64 = 4;      // (buf, max) -> this machine's IPv4 address, as text
    pub const TCP_CONNECT: u64 = 5;  // (host, port) -> connection id
    pub const TCP_SEND: u64 = 6;     // (id, data, len) -> bytes sent
    pub const TCP_RECV: u64 = 7;     // (id, buf, max, dont_wait) -> bytes read; 0 = closed; u64::MAX - 1 = nothing yet
    pub const TCP_CLOSE: u64 = 8;    // (id)
    pub const UDP_OPEN: u64 = 9;     // (port) -> socket id
    pub const UDP_SEND: u64 = 10;    // (id, to*, data, len) -> 0
    pub const UDP_RECV: u64 = 11;    // (id, buf, max, from*, dont_wait) -> bytes read; u64::MAX - 1 = nothing yet
    pub const UDP_CLOSE: u64 = 12;   // (id)
    pub const TCP_LISTEN: u64 = 13;  // (port) -> listener id
    pub const TCP_ACCEPT: u64 = 14;  // (id, from*, dont_wait) -> connection id; u64::MAX - 1 = nothing yet
    pub const TCP_LISTEN_CLOSE: u64 = 15; // (id)

    const_unique!(HTTP, HTTP_BODY, HTTP_HEADERS, PING, ADDRESS, TCP_CONNECT, TCP_SEND, TCP_RECV, TCP_CLOSE,
        UDP_OPEN, UDP_SEND, UDP_RECV, UDP_CLOSE, TCP_LISTEN, TCP_ACCEPT, TCP_LISTEN_CLOSE);
}

/// What a program passes for an HTTP request (libc's `http_request_t`):
/// method, URL and headers are NUL-terminated (method and headers may be
/// null), then the body and its length.
const HTTP_REQUEST_SIZE: usize = 5 * 8;

/// Where a datagram goes or came from (libc's `udp_addr_t`): a pointer to
/// a NUL-terminated host going out, and the IPv4 address (as a big-endian
/// number) coming in; then the port. An accepted connection's peer comes
/// in the same way.
const UDP_ADDR_SIZE: usize = 2 * 8;

/// Writes `addr` where a program asked for it (see UDP_ADDR_SIZE); `to`
/// was checked already, and may be null.
fn write_peer(to: *mut u64, addr: Option<core::net::SocketAddr>) {
    if let (false, Some(core::net::SocketAddr::V4(a))) = (to.is_null(), addr) {
        unsafe {
            to.write(u32::from(*a.ip()) as u64);
            to.add(1).write(a.port() as u64);
        }
    }
}

#[syscall(12)]
pub fn syscall_net(frame: &mut SyscallContext) {
    let syscall_num = frame[16];
    frame[0] = match syscall_num {
        net::HTTP => {
            let Some(raw) = user_ok(frame, frame[0], HTTP_REQUEST_SIZE, 8).then(|| unsafe {
                core::slice::from_raw_parts(frame[0] as *const u64, 5)
            }) else { frame[0] = fail("bad request"); return };
            let (method, url, headers, body, body_len) = (raw[0], raw[1], raw[2], raw[3], raw[4]);
            let method = if method == 0 { Some("GET") } else { user_cstr(frame, method) };
            let headers = if headers == 0 { Some("") } else { user_cstr(frame, headers) };
            let (Some(method), Some(url), Some(headers), Some(body)) =
                (method, user_cstr(frame, url), headers, user_bytes(frame, body, body_len))
            else { frame[0] = fail("bad request"); return };
            let opts = crate::net::RequestOptions {
                method: method.to_ascii_uppercase(),
                headers: headers.to_string(),
                body: body.to_vec(),
            };
            crate::process::set_http(None);
            match crate::net::fetch_with(url, opts).wait_unless_killed() {
                Some(Ok(resp)) => {
                    let mut text = alloc::string::String::new();
                    for (name, value) in &resp.headers {
                        text.push_str(&format!("{}: {}\n", name, value));
                    }
                    let status = resp.status as u64;
                    crate::process::set_http(Some(crate::process::HttpReply { body: resp.body, headers: text }));
                    status
                }
                Some(Err(e)) => fail(&e),
                None => u64::MAX,
            }
        }
        net::HTTP_BODY | net::HTTP_HEADERS => {
            // Check the buffer first: that takes the process table's lock,
            // which `with_http` holds.
            let Some(buf) = user_bytes_mut(frame, frame[0], frame[1]) else { frame[0] = fail("bad buffer"); return };
            crate::process::with_http(|reply| {
                let data = match reply {
                    Some(r) if syscall_num == net::HTTP_BODY => &r.body[..],
                    Some(r) => r.headers.as_bytes(),
                    None => return u64::MAX,
                };
                let n = data.len().min(buf.len());
                buf[..n].copy_from_slice(&data[..n]);
                data.len() as u64
            }).unwrap_or(u64::MAX)
        }
        net::PING => {
            let Some(host) = user_cstr(frame, frame[0]) else { frame[0] = fail("bad host name"); return };
            match crate::net::ping(host).wait_unless_killed() {
                Some(Ok(ms)) => ms,
                Some(Err(e)) => fail(&e),
                None => u64::MAX,
            }
        }
        net::ADDRESS => match crate::net::status().address {
            Some(cidr) => copy_out(frame, frame[0], frame[1], format!("{}", cidr.address()).as_bytes()),
            None => fail("no network address yet"),
        },
        net::TCP_CONNECT => {
            let Some(host) = user_cstr(frame, frame[0]) else { frame[0] = fail("bad host name"); return };
            let Ok(port) = u16::try_from(frame[1]) else { frame[0] = fail("bad port"); return };
            crate::net::tcp_connect(host, port).unwrap_or_else(|e| fail(&e))
        }
        net::TCP_SEND => {
            let Some(data) = user_bytes(frame, frame[1], frame[2]) else { frame[0] = fail("bad buffer"); return };
            match crate::net::tcp_send(frame[0], data) {
                Ok(n) => n as u64,
                Err(e) => fail(&e),
            }
        }
        net::TCP_RECV => {
            let (id, dont_wait) = (frame[0], frame[3] != 0);
            let Some(buf) = user_bytes_mut(frame, frame[1], frame[2]) else { frame[0] = fail("bad buffer"); return };
            match crate::net::tcp_recv(id, buf, !dont_wait) {
                Ok(Some(n)) => n as u64,
                Ok(None) => u64::MAX - 1,
                Err(e) => fail(&e),
            }
        }
        net::TCP_CLOSE => {
            crate::net::tcp_close(frame[0]);
            0
        }
        net::UDP_OPEN => {
            let Ok(port) = u16::try_from(frame[0]) else { frame[0] = fail("bad port"); return };
            crate::net::udp_open(port).unwrap_or_else(|e| fail(&e))
        }
        net::UDP_SEND => {
            let Some(to) = user_ok(frame, frame[1], UDP_ADDR_SIZE, 8).then(|| unsafe {
                core::slice::from_raw_parts(frame[1] as *const u64, 2)
            }) else { frame[0] = fail("bad address"); return };
            let (Some(host), Ok(port)) = (user_cstr(frame, to[0]), u16::try_from(to[1]))
            else { frame[0] = fail("bad address"); return };
            let Some(data) = user_bytes(frame, frame[2], frame[3]) else { frame[0] = fail("bad buffer"); return };
            match crate::net::udp_send(frame[0], host, port, data) {
                Ok(()) => 0,
                Err(e) => fail(&e),
            }
        }
        net::UDP_RECV => {
            let (id, from, dont_wait) = (frame[0], frame[3], frame[4] != 0);
            let Some(buf) = user_bytes_mut(frame, frame[1], frame[2]) else { frame[0] = fail("bad buffer"); return };
            let Some(from) = (from == 0 || user_ok(frame, from, UDP_ADDR_SIZE, 8)).then_some(from as *mut u64)
            else { frame[0] = fail("bad address"); return };
            match crate::net::udp_recv(id, !dont_wait) {
                Ok(Some(d)) => {
                    write_peer(from, Some(d.from));
                    // A datagram longer than the buffer is cut short.
                    let n = d.data.len().min(buf.len());
                    buf[..n].copy_from_slice(&d.data[..n]);
                    n as u64
                }
                Ok(None) => u64::MAX - 1,
                Err(e) => fail(&e),
            }
        }
        net::UDP_CLOSE => {
            crate::net::udp_close(frame[0]);
            0
        }
        net::TCP_LISTEN => {
            let Ok(port) = u16::try_from(frame[0]) else { frame[0] = fail("bad port"); return };
            crate::net::tcp_listen(port).unwrap_or_else(|e| fail(&e))
        }
        net::TCP_ACCEPT => {
            let (id, from, dont_wait) = (frame[0], frame[1], frame[2] != 0);
            let Some(from) = (from == 0 || user_ok(frame, from, UDP_ADDR_SIZE, 8)).then_some(from as *mut u64)
            else { frame[0] = fail("bad address"); return };
            match crate::net::tcp_accept(id, !dont_wait) {
                Ok(Some((conn, peer))) => {
                    write_peer(from, peer);
                    conn
                }
                Ok(None) => u64::MAX - 1,
                Err(e) => fail(&e),
            }
        }
        net::TCP_LISTEN_CLOSE => {
            crate::net::tcp_listen_close(frame[0]);
            0
        }
        _ => { log::warn!("net syscall: unknown sub-command {}", syscall_num); u64::MAX }
    };
}

// ---- processes -----------------------------------------------------------------

mod proc {
    use kos_derive::const_unique;

    pub const SPAWN: u64 = 0;      // (path, args) -> task id; it gets the caller's stdin, stdout and stderr
    pub const WAIT: u64 = 1;       // (id) -> its exit status, once it has ended
    pub const KILL: u64 = 2;       // (id) -> 0
    pub const SELF_ID: u64 = 3;    // -> the caller's task id
    pub const RUNNING: u64 = 4;    // (id) -> 1 if it's a program still running, else 0
    pub const LAST_ERROR: u64 = 5; // (buf, max) -> why the last call failed, as text
    pub const SPAWN_IO: u64 = 6;   // (path, args, int fds[3]) -> task id; its stdin, stdout and stderr are the caller's fds[0..3] (-1: the same one)
    pub const TASKS: u64 = 7;      // (buf, max) -> length of a line per task: "id core state cpu% kind name", tab-separated, then "core N idle%" lines

    const_unique!(SPAWN, WAIT, KILL, SELF_ID, RUNNING, LAST_ERROR, SPAWN_IO, TASKS);
}

#[syscall(13)]
pub fn syscall_process(frame: &mut SyscallContext) {
    let syscall_num = frame[16];
    frame[0] = match syscall_num {
        proc::SPAWN | proc::SPAWN_IO => {
            let args = if frame[1] == 0 { Some("") } else { user_cstr(frame, frame[1]) };
            let (Some(path), Some(args)) = (user_cstr(frame, frame[0]), args) else { frame[0] = fail("bad path"); return };
            // Which of the caller's descriptors become the program's 0-2.
            let mut from = [0i32, 1, 2];
            if syscall_num == proc::SPAWN_IO {
                if !user_ok(frame, frame[2], 12, 4) {
                    frame[0] = fail("bad fds");
                    return;
                }
                let given = unsafe { (frame[2] as *const [i32; 3]).read() };
                for i in 0..3 {
                    if given[i] >= 0 {
                        from[i] = given[i];
                    }
                }
            }
            let stdio = if frame.is_user() {
                let fds = from.map(|n| crate::process::fd_get(n as u64));
                if syscall_num == proc::SPAWN_IO && fds.iter().any(Option::is_none) {
                    frame[0] = fail("bad file descriptor");
                    return;
                }
                Some(fds.map(|f| f.unwrap_or_else(crate::fd::null)))
            } else {
                None
            };
            // A program it starts never takes the keyboard from it.
            let console = match crate::process::console() {
                Some(Console::Detached) => Console::Detached,
                _ => Console::Output,
            };
            match crate::loader::spawn(path, args, console, stdio) {
                Ok(id) => id as u64,
                Err(e) => fail(&format!("{}: {}", path, e)),
            }
        }
        proc::WAIT => {
            let id = frame[0] as scheduler::TaskId;
            if id == scheduler::current_id() {
                frame[0] = fail("a program can't wait for itself");
                return;
            }
            crate::process::EXITED.wait_until(|| crate::process::exit_status(id).is_some() || scheduler::current_killed());
            match crate::process::exit_status(id) {
                Some(Some(status)) => status as u64,
                Some(None) => fail("no such program"),
                None => u64::MAX,
            }
        }
        proc::KILL => match crate::process::kill(frame[0] as scheduler::TaskId) {
            Ok(()) => 0,
            Err(e) => fail(e),
        },
        proc::SELF_ID => scheduler::current_id() as u64,
        proc::RUNNING => crate::process::is_running(frame[0] as scheduler::TaskId) as u64,
        proc::LAST_ERROR => {
            let why = crate::process::last_error().unwrap_or_default();
            copy_out(frame, frame[0], frame[1], why.as_bytes())
        }
        proc::TASKS => copy_out(frame, frame[0], frame[1], scheduler::tasks_text().as_bytes()),
        _ => { log::warn!("process syscall: unknown sub-command {}", syscall_num); u64::MAX }
    };
}

#[syscall(9)]
pub fn syscall_yield(frame: &mut SyscallContext) {
    frame.request = Request::Yield as u64;
}

#[syscall(8)]
pub fn syscall_dump(_frame: &mut SyscallContext) {
    if !may_print() {
        return;
    }
    let dump = crate::arch::stacktrace::RegisterDump::capture();
    println!("{}", dump);
}

// ---- system info ---------------------------------------------------------------

/// Bytes per `utsname_t` field, NUL included (sys.h's UTS_LEN).
const UTS_LEN: usize = 65;

/// `uname(buf)`: fills `buf` (a `utsname_t`: five NUL-terminated fields of
/// UTS_LEN bytes) with the system's name, node, release, version and
/// machine. Returns 0.
#[syscall(14)]
pub fn syscall_uname(frame: &mut SyscallContext) {
    let fields = crate::util::SYSTEM_UNAME;
    let Some(buf) = user_bytes_mut(frame, frame[0], (fields.len() * UTS_LEN) as u64) else {
        frame[0] = fail("bad buffer");
        return;
    };
    buf.fill(0);
    for (field, value) in buf.chunks_mut(UTS_LEN).zip(fields) {
        let n = value.len().min(UTS_LEN - 1);
        field[..n].copy_from_slice(&value.as_bytes()[..n]);
    }
    frame[0] = 0;
}

// ---- sound ---------------------------------------------------------------------
// One voice: each call replaces whatever was playing (see crate::sound).
// A program's sound stops when it ends, so `play` and `beep` wait for
// theirs to finish (or be replaced), like TempleOS's.

mod sound {
    use kos_derive::const_unique;

    pub const SND: u64 = 0;   // (hz) -> 0; 0 Hz is silence
    pub const BEEP: u64 = 1;  // -> 0, once it's over
    pub const PLAY: u64 = 2;  // (notes, len) -> 0, once the song is over
    pub const STOP: u64 = 3;  // -> 0
    pub const BUSY: u64 = 4;  // -> 1 if something is playing, else 0
    pub const PLAY_FILE: u64 = 5; // (path, len) -> 0, once the WAV file is over

    const_unique!(SND, BEEP, PLAY, STOP, BUSY, PLAY_FILE);
}

#[syscall(16)]
pub fn syscall_sound(frame: &mut SyscallContext) {
    // Waits for sound `id` to finish, unless the program is killed meanwhile.
    let wait = |id: u64| {
        while !crate::sound::finished(id) && !scheduler::current_killed() {
            crate::arch::timer::delay_ms(10);
        }
    };
    let result = match frame[16] {
        sound::SND => crate::sound::snd(frame[0].min(u32::MAX as u64) as u32),
        sound::BEEP => crate::sound::beep().map(wait),
        sound::PLAY => match user_str(frame, frame[0], frame[1]) {
            Some(notes) => crate::sound::play(notes).map(wait),
            None => Err("bad notes".to_string()),
        },
        sound::STOP => crate::sound::stop(),
        sound::PLAY_FILE => match user_str(frame, frame[0], frame[1]) {
            Some(path) => crate::sound::play_file(&current_cwd(), path).map(wait),
            None => Err("bad path".to_string()),
        },
        sound::BUSY => {
            frame[0] = crate::sound::busy() as u64;
            return;
        }
        _ => {
            log::warn!("sound syscall: unknown sub-command {}", frame[16]);
            frame[0] = fail("unknown sound call");
            return;
        }
    };
    frame[0] = match result {
        Ok(()) => 0,
        Err(why) => fail(&why),
    };
}

// ---- the GPU ----------------------------------------------------------------------

mod gpu {
    use kos_derive::const_unique;

    pub const AVAILABLE: u64 = 0; // () -> 1 if there's a kos-gl device, else 0
    pub const OPEN: u64 = 1;      // (width, height) -> context
    pub const SUBMIT: u64 = 2;    // (context, commands, len, frame or 0) -> 0
    pub const HLSL: u64 = 3;      // (context | program << 32, request, len, log, log_max) -> 0, or 1 with the log
    pub const CLOSE: u64 = 4;     // (context)
    pub const GLSL: u64 = 5;      // as HLSL; program 0 only checks the shaders

    const_unique!(AVAILABLE, OPEN, SUBMIT, HLSL, CLOSE, GLSL);
}

/// GL contexts on the host's GPU, for gl.h (see drivers/virtio/kgl.rs).
/// A program can only use contexts it opened, and they're closed for it
/// when it ends. Errors return u64::MAX, with the reason kept for
/// `last_error`.
#[syscall(18)]
pub fn syscall_gpu(frame: &mut SyscallContext) {
    use crate::drivers::virtio::kgl;
    let ctx = frame[0] as u32;
    let result: Result<u64, alloc::string::String> = match frame[16] {
        gpu::AVAILABLE => Ok(kgl::available() as u64),
        gpu::OPEN => kgl::open(frame[0].min(u32::MAX as u64) as u32, frame[1].min(u32::MAX as u64) as u32).map(u64::from),
        gpu::SUBMIT => match (user_bytes(frame, frame[1], frame[2]), kgl::size(ctx)) {
            (Some(commands), Some((w, h))) => {
                let frame_len = w as u64 * h as u64 * 4;
                match frame[3] {
                    0 => kgl::submit(ctx, commands, None).map(|()| 0),
                    ptr => match user_bytes_mut(frame, ptr, frame_len) {
                        Some(pixels) => kgl::submit(ctx, commands, Some(pixels)).map(|()| 0),
                        None => Err("bad frame buffer".to_string()),
                    },
                }
            }
            (None, _) => Err("bad commands".to_string()),
            (_, None) => Err("no such context".to_string()),
        },
        gpu::HLSL | gpu::GLSL => match user_bytes(frame, frame[1], frame[2]) {
            Some(request) => match frame[16] {
                gpu::HLSL => kgl::hlsl(ctx, (frame[0] >> 32) as u32, request),
                _ => kgl::glsl(ctx, (frame[0] >> 32) as u32, request),
            }
            .and_then(|log| match log {
                None => Ok(0),
                Some(log) => {
                    // NUL-terminated, cut short if it doesn't fit.
                    let mut text = log.into_bytes();
                    let max = frame[4] as usize;
                    if max > 0 {
                        text.truncate(max - 1);
                        text.push(0);
                    }
                    match copy_out(frame, frame[3], frame[4], &text) {
                        u64::MAX => Err("bad log buffer".to_string()),
                        _ => Ok(1),
                    }
                }
            }),
            None => Err("bad shaders".to_string()),
        },
        gpu::CLOSE => {
            kgl::close(ctx);
            Ok(0)
        }
        _ => {
            log::warn!("gpu syscall: unknown sub-command {}", frame[16]);
            Err("unknown gpu call".to_string())
        }
    };
    frame[0] = result.unwrap_or_else(|why| fail(&why));
}

fn find_syscall(num: u64) -> Option<&'static SyscallEntry> {
    unsafe {
        let mut ptr = core::ptr::addr_of!(__syscalls_start);
        let end = core::ptr::addr_of!(__syscalls_end);
        while ptr < end {
            let entry = &*ptr;
            if entry.number == num {
                return Some(entry);
            }
            ptr = ptr.add(1);
        }
    }
    None
}

/// The handler's name, without the `syscall_` prefix.
pub fn syscall_name(num: u64) -> &'static str {
    find_syscall(num).map_or("unknown", |e| e.name.trim_start_matches("syscall_"))
}

pub fn dispatch_syscall(num: u64, frame: &mut SyscallContext) -> bool {
    let Some(entry) = find_syscall(num) else {
        log::warn!("unknown syscall {} from elr {:#x}", num, frame.elr);
        return false;
    };
    if let Some(counter) = crate::stats::SYSCALLS.get(num as usize) {
        crate::stats::inc(counter);
    }
    // Kernel tasks block by yielding, many times a second: that's the
    // scheduler's business (target `scheduler`), not syscall tracing.
    if num == SYS_YIELD {
        (entry.handler)(frame);
        return true;
    }
    let name = entry.name.trim_start_matches("syscall_");
    // Syscalls 4-7, 12-13 and 15-18 take a sub-command in x16.
    if (4..=7).contains(&num) || (12..=13).contains(&num) || (15..=18).contains(&num) {
        log::trace!("{} (svc {}) sub={} x0={:#x} x1={:#x} x2={:#x} x3={:#x}", name, num, frame[16], frame[0], frame[1], frame[2], frame[3]);
    } else {
        log::trace!("{} (svc {}) x0={:#x} x1={:#x} x2={:#x} x3={:#x}", name, num, frame[0], frame[1], frame[2], frame[3]);
    }
    (entry.handler)(frame);
    log::trace!("{} -> x0={:#x}", name, frame[0]);
    true
}
