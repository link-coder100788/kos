use crate::filesys::backend::FsBackend;
use crate::filesys::backend::FsError;
use crate::filesys::fs_ipc::{FsRequest, FsResponse, FS_REQUEST_QUEUE, FS_REQUEST_WAIT, FS_RESPONSE_QUEUE, FS_RESPONSE_WAIT};
use crate::drivers::virtio::blk::BlkDevice;
use crate::println;
use crate::scheduler::wait::WaitQueue;
use core::sync::atomic::{AtomicBool, Ordering};

// Swap which filesystem backend is active by commenting/uncommenting one
// of these two lines. Everything else (fs_task, ShellCommand, fs_ipc) talks
// to the FsBackend trait and doesn't know or care which concrete type is
// behind it.
//type ActiveFs = crate::filesys::simplefs::FileSystem;
type ActiveFs = crate::filesys::fatfs_backend::FatFsBackend;

/// The filesystem task. Owns the BlkDevice and the FileSystem exclusively --
/// no other task should touch either. All disk I/O happens here, serialized
/// one request at a time by construction (the caller blocks on fs_call).
static READY: AtomicBool = AtomicBool::new(false);
static READY_WAIT: WaitQueue = WaitQueue::new();

/// Blocks until fs_task has mounted the disk (or found there isn't one).
pub fn wait_until_ready() {
    READY_WAIT.wait_until(|| READY.load(Ordering::Acquire));
}

pub extern "C" fn fs_task() -> ! {
    let mut fs = match BlkDevice::init() {
        Some(blk) => {
            let fs = ActiveFs::mount_or_format(blk);
            log::info!("filesystem ready");
            Some(fs)
        }
        None => {
            log::warn!("no block device; every file request will fail");
            None
        }
    };

    READY.store(true, Ordering::Release);
    READY_WAIT.wake_all();

    loop {
        let mut req = None;
        FS_REQUEST_WAIT.wait_until(|| {
            req = FS_REQUEST_QUEUE.pop();
            req.is_some()
        });
        let req = req.unwrap();
        let summary = describe(&req);
        let started = crate::arch::timer::uptime_ms();
        crate::stats::inc(&crate::stats::FS_REQUESTS);
        let resp = match &mut fs {
            Some(fs) => handle(fs, req),
            None => FsResponse::Err(FsError::Io),
        };
        let ms = crate::arch::timer::uptime_ms() - started;
        match &resp {
            FsResponse::Err(e) => log::debug!("{} -> {:?} ({} ms)", summary, e, ms),
            FsResponse::Data(d) => log::debug!("{} -> {} bytes ({} ms)", summary, d.len(), ms),
            FsResponse::Listing(l) => log::debug!("{} -> {} entries ({} ms)", summary, l.len(), ms),
            _ => log::debug!("{} -> ok ({} ms)", summary, ms),
        }
        // fs_call only ever has one request in flight, so there's room.
        if FS_RESPONSE_QUEUE.push(resp).is_err() {
            panic!("fs_task: response queue full");
        }
        FS_RESPONSE_WAIT.wake_all();
    }
}

/// One-line description of a request, for the log.
fn describe(req: &FsRequest) -> alloc::string::String {
    use alloc::format;
    match req {
        FsRequest::List { cwd, path } => format!("list {} (in {})", path, cwd),
        FsRequest::ChangeDir { cwd, path } => format!("cd {} (in {})", path, cwd),
        FsRequest::Read { cwd, path } => format!("read {} (in {})", path, cwd),
        FsRequest::Write { cwd, path, data } => format!("write {} bytes to {} (in {})", data.len(), path, cwd),
        FsRequest::Mkdir { cwd, path } => format!("mkdir {} (in {})", path, cwd),
        FsRequest::Create { cwd, path } => format!("create {} (in {})", path, cwd),
        FsRequest::Remove { cwd, path } => format!("remove {} (in {})", path, cwd),
    }
}

fn handle(fs: &mut ActiveFs, req: FsRequest) -> FsResponse {
    match req {
        FsRequest::List { cwd, path } => match fs.list_dir(&cwd, &path) {
            Ok(entries) => FsResponse::Listing(entries),
            Err(e) => FsResponse::Err(e),
        },
        FsRequest::ChangeDir { cwd, path } => match fs.change_dir(&cwd, &path) {
            Ok(new_path) => FsResponse::Path(new_path),
            Err(e) => FsResponse::Err(e),
        },
        FsRequest::Read { cwd, path } => match fs.read_file(&cwd, &path) {
            Ok(data) => FsResponse::Data(data),
            Err(e) => FsResponse::Err(e),
        },
        FsRequest::Write { cwd, path, data } => match fs.write_file(&cwd, &path, &data) {
            Ok(()) => FsResponse::Ok,
            Err(e) => FsResponse::Err(e),
        },
        FsRequest::Mkdir { cwd, path } => match fs.mkdir(&cwd, &path) {
            Ok(()) => FsResponse::Ok,
            Err(e) => FsResponse::Err(e),
        },
        FsRequest::Create { cwd, path } => match fs.create_file(&cwd, &path) {
            Ok(()) => FsResponse::Ok,
            Err(e) => FsResponse::Err(e),
        },
        FsRequest::Remove { cwd, path } => match fs.remove(&cwd, &path) {
            Ok(()) => FsResponse::Ok,
            Err(e) => FsResponse::Err(e),
        },
    }
}
