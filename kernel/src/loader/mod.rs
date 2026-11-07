//! Program loading: `run`/`bg` push a job onto `JOBS`, and the loader task
//! (`user_task_runner_1`) reads the file, loads it into a new process (see
//! `process`), and starts it. ELF programs are stored base64-encoded
//! (`.elf.b64`); K programs (`.kbin`) are loaded as they are.
//!
//! The loader task also cleans up after programs that have exited.

pub mod elf;

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use base64::Engine;
use crate::filesys::fs_ipc::FsRequest;
use crate::filesys::fs_ipc::FsResponse;
use crate::filesys::fs_ipc::fs_call;
use crate::println;
use crate::fd::Fd;
use crate::process::{self, Console, NewProcess, USER_BASE, HEAP_LIMIT};
use crate::scheduler::TaskId;
use crate::shell::fs::current_cwd;
use crate::std::lock::irq_mutex::IrqMutex;
use self::elf::load_elf;

struct Job {
    path: String,
    args: String,
    console: Console,
    /// Its stdin, stdout and stderr, if not the console's.
    stdio: Option<[Fd; 3]>,
}

static JOBS: IrqMutex<VecDeque<Job>> = IrqMutex::new(VecDeque::new());
/// A `run` job hasn't been started (or failed to start) yet.
static FOREGROUND_PENDING: AtomicBool = AtomicBool::new(false);

fn queue(path: String, args: String, console: Console) {
    JOBS.lock().push_back(Job { path, args, console, stdio: None });
    // The loader task waits on the same queue as programs exiting.
    process::EXITED.wake_all();
}

/// Starts a program whose output goes to the Terminal, without waiting
/// for it (the Programs window).
pub fn run(path: String) {
    queue(path, String::new(), Console::Output);
}

/// Starts a program detached from the console (`bg`), with `args` as its
/// command-line arguments.
pub fn run_background(path: String, args: String) {
    queue(path, args, Console::Detached);
}

/// Runs a program in the foreground (`run`, or a command from `/bin`): it
/// gets the console, and this returns once it has exited, or Ctrl+C has
/// ended it.
pub fn run_foreground(path: String, args: String) {
    process::take_interrupted();
    FOREGROUND_PENDING.store(true, Ordering::SeqCst);
    queue(path, args, Console::Foreground);
    process::EXITED.wait_until(|| !FOREGROUND_PENDING.load(Ordering::SeqCst) && !process::foreground_running());
    if process::take_interrupted() {
        println!("^C");
    }
}

/// Runs a pipeline in the foreground: each (path, args, stdin/stdout/
/// stderr) is started at once, and this returns once they've all ended,
/// or Ctrl+C has ended them.
pub fn run_pipeline(programs: Vec<(String, String, [Fd; 3])>) {
    process::take_interrupted();
    for (path, args, stdio) in programs {
        // Dropping a program that didn't start closes its ends of the
        // pipes, so its neighbours see the end of their input.
        if let Err(e) = spawn(&path, &args, Console::Foreground, Some(stdio)) {
            println!("can't run {}: {}", path, e);
        }
    }
    process::EXITED.wait_until(|| !process::foreground_running());
    if process::take_interrupted() {
        println!("^C");
    }
}

fn read_program(path: &str) -> Result<Vec<u8>, &'static str> {
    let bytes = match fs_call(FsRequest::Read { cwd: current_cwd(), path: String::from(path) }) {
        FsResponse::Data(bytes) => bytes,
        _ => return Err("can't read it"),
    };
    if path.ends_with(".kbin") {
        return Ok(bytes);
    }
    if path.ends_with(".elf.b64") {
        let mut elf = vec![0u8; bytes.len()];
        let len = base64::engine::general_purpose::STANDARD.decode_slice(&bytes, &mut elf).map_err(|_| "it isn't base64")?;
        elf.truncate(len);
        return Ok(elf);
    }
    if path.ends_with(".elf") {
        return Ok(bytes);
    }
    Err("invalid file")
}

/// Loads and starts one job, or says why it couldn't.
fn start(job: &Job) -> Result<TaskId, String> {
    let image = read_program(&job.path).map_err(String::from)?;
    log::debug!("{}: {} byte image", job.path, image.len());
    let name = job.path.rsplit('/').next().unwrap_or(&job.path);
    let name = name.trim_end_matches(".b64").trim_end_matches(".elf").trim_end_matches(".kbin");
    start_image(name, &job.args, &image, job.path.ends_with(".kbin"), job.console, job.stdio.clone())
}

/// Loads and starts program `path` straight away (for a program's
/// `spawn`), returning its task id. `stdio` is its stdin, stdout and
/// stderr; without it, it gets what `console` says.
pub fn spawn(path: &str, args: &str, console: Console, stdio: Option<[Fd; 3]>) -> Result<TaskId, String> {
    start(&Job { path: String::from(path), args: String::from(args), console, stdio })
}

/// Somewhere to load a program: the ELF and K loaders write through this.
/// Addresses are the program's own.
pub trait ProgramMemory {
    /// Makes `va..va+len` usable, zero-filled where nothing is there yet.
    fn map(&mut self, va: usize, len: usize) -> Result<(), &'static str>;
    /// Copies `bytes` to `va`, which must be mapped.
    fn write(&mut self, va: usize, bytes: &[u8]) -> Result<(), &'static str>;
}

/// Starts a program from an ELF (or, if `kbin`, a K program) in memory.
pub fn start_image(name: &str, args: &str, image: &[u8], kbin: bool, console: Console, stdio: Option<[Fd; 3]>) -> Result<TaskId, String> {
    use alloc::format;
    let mut new = NewProcess::new(name, console).ok_or("out of memory")?;
    new.set_args(args);
    if let Some(stdio) = stdio {
        new.set_stdio(stdio);
    }
    let loaded = if kbin {
        crate::k::load::load_kbin(image, USER_BASE..HEAP_LIMIT, &mut new).map_err(|e| format!("bad K program: {:?}", e))
    } else {
        load_elf(image, USER_BASE, USER_BASE..HEAP_LIMIT, &mut new).map_err(|e| format!("bad ELF: {:?}", e))
    };
    // On failure, dropping `new` frees whatever was mapped.
    let (entry, image_end) = loaded?;
    let id = process::start(new, entry, image_end)?;
    log::debug!("{}: task {}, image {:#x}..{:#x}, {}", name, id, USER_BASE, image_end, console.name());
    Ok(id)
}

pub extern "C" fn user_task_runner_1() -> ! {
    loop {
        let mut job = None;
        process::EXITED.wait_until(|| {
            job = JOBS.lock().pop_front();
            job.is_some() || process::needs_reaping()
        });
        if process::needs_reaping() {
            process::reap();
        }
        let Some(job) = job else { continue };
        log::info!("loading {}", job.path);
        if let Err(e) = start(&job) {
            log::error!("{}: {}", job.path, e);
            println!("can't run {}: {}", job.path, e);
        }
        if job.console == Console::Foreground {
            FOREGROUND_PENDING.store(false, Ordering::SeqCst);
            process::EXITED.wake_all();
        }
    }
}
