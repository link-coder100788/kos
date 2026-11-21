use crate::filesys::backend::FsBackend;
use crate::filesys::backend::FsError;
use crate::filesys::perms::{self, Attrs, Perms, PERMS_FILE};
use crate::filesys::fs_ipc::{FsRequest, FsResponse, FS_REQUEST_QUEUE, FS_REQUEST_WAIT, FS_RESPONSE_QUEUE, FS_RESPONSE_WAIT};
use crate::drivers::virtio::blk::BlkDevice;
use crate::println;
use crate::scheduler::wait::WaitQueue;
use crate::users::{Uid, ROOT};
use core::sync::atomic::{AtomicBool, Ordering};
use alloc::string::String;

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

    let mut perms = match &mut fs {
        Some(fs) => load_perms(fs),
        None => Perms::new(),
    };

    READY.store(true, Ordering::Release);
    READY_WAIT.wake_all();

    loop {
        let mut req = None;
        FS_REQUEST_WAIT.wait_until(|| {
            req = FS_REQUEST_QUEUE.pop();
            req.is_some()
        });
        let (uid, req) = req.unwrap();
        let summary = alloc::format!("{} as {}", describe(&req), uid);
        let started = crate::arch::timer::uptime_ms();
        crate::stats::inc(&crate::stats::FS_REQUESTS);
        let resp = match &mut fs {
            Some(fs) => handle(fs, &mut perms, uid, req),
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
        FsRequest::Chmod { cwd, path, read, write } => format!("chmod r={:?} w={:?} {} (in {})", read, write, path, cwd),
        FsRequest::Chown { cwd, path, uid } => format!("chown {} {} (in {})", uid, path, cwd),
        FsRequest::Attrs { cwd, path } => format!("attrs {} (in {})", path, cwd),
        FsRequest::MayWrite { cwd, path } => format!("may write {} (in {})", path, cwd),
    }
}

fn load_perms(fs: &mut ActiveFs) -> Perms {
    match fs.read_file("/", PERMS_FILE) {
        Ok(bytes) => {
            let perms = Perms::parse(&String::from_utf8_lossy(&bytes));
            log::info!("loaded {}", PERMS_FILE);
            perms
        }
        Err(_) => Perms::new(),
    }
}

fn save_perms(fs: &mut ActiveFs, perms: &mut Perms) {
    let _ = fs.mkdir("/", "/.kos");
    // Only root sees what's private.
    perms.set(PERMS_FILE, Attrs { owner: ROOT, read: false, write: false });
    if let Err(e) = fs.write_file("/", PERMS_FILE, perms.to_text().as_bytes()) {
        log::error!("can't save {}: {:?}", PERMS_FILE, e);
    }
}

/// Whether `path` (normalized) is there: its folder lists it.
fn exists(fs: &mut ActiveFs, path: &str) -> bool {
    if path == "/" {
        return true;
    }
    let name = &path[path.rfind('/').map_or(0, |i| i + 1)..];
    fs.list_dir("/", perms::parent(path))
        .is_ok_and(|entries| entries.iter().any(|e| e.name.eq_ignore_ascii_case(name)))
}

fn status(result: Result<(), FsError>) -> FsResponse {
    match result {
        Ok(()) => FsResponse::Ok,
        Err(e) => FsResponse::Err(e),
    }
}

/// Checks `req` against `uid`'s permissions, carries it out, and keeps
/// the permission entries up to date (see `filesys::perms`).
fn handle(fs: &mut ActiveFs, perms: &mut Perms, uid: Uid, req: FsRequest) -> FsResponse {
    let (cwd, path) = match &req {
        FsRequest::List { cwd, path } | FsRequest::ChangeDir { cwd, path } | FsRequest::Read { cwd, path }
        | FsRequest::Write { cwd, path, .. } | FsRequest::Mkdir { cwd, path } | FsRequest::Create { cwd, path }
        | FsRequest::Remove { cwd, path } | FsRequest::Chmod { cwd, path, .. } | FsRequest::Chown { cwd, path, .. }
        | FsRequest::Attrs { cwd, path } | FsRequest::MayWrite { cwd, path } => (cwd, path),
    };
    let target = perms::normalize(cwd, path);
    let allowed = match &req {
        FsRequest::List { .. } | FsRequest::ChangeDir { .. } | FsRequest::Read { .. } => perms.may_read(uid, &target),
        FsRequest::Write { .. } | FsRequest::Mkdir { .. } | FsRequest::Create { .. } | FsRequest::MayWrite { .. } => perms.may_write(uid, &target),
        FsRequest::Remove { .. } => perms.may_remove(uid, &target),
        FsRequest::Chmod { .. } => perms.may_chmod(uid, &target),
        FsRequest::Chown { .. } => uid == ROOT,
        FsRequest::Attrs { .. } => perms.may_read(uid, perms::parent(&target)),
    };
    if !allowed {
        return FsResponse::Err(FsError::PermissionDenied);
    }
    // Something new in a folder its creator doesn't own becomes theirs;
    // writing over a file that's already there doesn't change its owner.
    let creates = matches!(req, FsRequest::Write { .. } | FsRequest::Mkdir { .. } | FsRequest::Create { .. })
        && perms.attrs(perms::parent(&target)).owner != uid && !perms.has_entry(&target) && !exists(fs, &target);
    let resp = handle_allowed(fs, perms, req, &target);
    if creates && matches!(resp, FsResponse::Ok) && perms.created(uid, &target) {
        save_perms(fs, perms);
    }
    resp
}

/// Carries out a request that's been allowed; `target` is its path,
/// normalized.
fn handle_allowed(fs: &mut ActiveFs, perms: &mut Perms, req: FsRequest, target: &str) -> FsResponse {
    match req {
        FsRequest::List { cwd, path } => match fs.list_dir(&cwd, &path) {
            Ok(entries) => FsResponse::Listing(entries),
            Err(e) => FsResponse::Err(e),
        },
        FsRequest::ChangeDir { cwd, path } => match fs.change_dir(&cwd, &path) {
            Ok(new_path) => FsResponse::Path(new_path),
            // FAT says "not a folder" for one that isn't there at all.
            Err(FsError::NotADirectory) if !exists(fs, target) => FsResponse::Err(FsError::NotFound),
            Err(e) => FsResponse::Err(e),
        },
        FsRequest::Read { cwd, path } => match fs.read_file(&cwd, &path) {
            Ok(data) => FsResponse::Data(data),
            Err(e) => FsResponse::Err(e),
        },
        FsRequest::Write { cwd, path, data } => {
            let result = fs.write_file(&cwd, &path, &data);
            // Root edited the entries by hand.
            if result.is_ok() && target == PERMS_FILE {
                *perms = load_perms(fs);
            }
            status(result)
        }
        FsRequest::Mkdir { cwd, path } => match fs.mkdir(&cwd, &path) {
            Ok(()) => FsResponse::Ok,
            Err(e) => FsResponse::Err(e),
        },
        FsRequest::Create { cwd, path } => match fs.create_file(&cwd, &path) {
            Ok(()) => FsResponse::Ok,
            Err(e) => FsResponse::Err(e),
        },
        FsRequest::Remove { cwd, path } => {
            let result = fs.remove(&cwd, &path);
            if result.is_ok() && perms.removed(target) {
                save_perms(fs, perms);
            }
            status(result)
        }
        FsRequest::Chmod { read, write, .. } => {
            if !exists(fs, target) {
                return FsResponse::Err(FsError::NotFound);
            }
            let old = perms.attrs(target);
            perms.set(target, Attrs { owner: old.owner, read: read.unwrap_or(old.read), write: write.unwrap_or(old.write) });
            save_perms(fs, perms);
            FsResponse::Ok
        }
        FsRequest::Chown { uid, .. } => {
            if !exists(fs, target) {
                return FsResponse::Err(FsError::NotFound);
            }
            let old = perms.attrs(target);
            perms.set(target, Attrs { owner: uid, ..old });
            save_perms(fs, perms);
            FsResponse::Ok
        }
        FsRequest::Attrs { .. } => match exists(fs, target) {
            true => FsResponse::Attrs(perms.attrs(target)),
            false => FsResponse::Err(FsError::NotFound),
        },
        FsRequest::MayWrite { .. } => FsResponse::Ok,
    }
}
