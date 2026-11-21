//! User accounts. Each account has a name, a uid (root is 0, everyone
//! else from 1000), a home folder, and an Argon2id password hash (see
//! `crypto::argon`). They're kept in `/.kos/users`, a line each:
//! `name:uid:home:<PHC hash>`, which only root can read.
//!
//! Every task acts as some uid (`scheduler::current_uid`), and fs_task
//! checks each request against it (see `filesys::perms`). Programs act as
//! whoever started them. The console shell, the desktop and K-AI act as
//! the session's user: one session, shared by the serial console and the
//! Terminal, changed by logging in, `su` and `logout`.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use crate::filesys::backend::FsError;
use crate::filesys::fs_ipc::{fs_call_as, FsRequest, FsResponse};

pub type Uid = u32;

/// Root may do anything.
pub const ROOT: Uid = 0;
/// Before anyone logs in: can read what everyone can, nothing else.
pub const NOBODY: Uid = 65534;
/// The first uid `add` hands out.
const FIRST_USER: Uid = 1000;

pub const USERS_FILE: &str = "/.kos/users";

/// Who the session acts as.
static SESSION: AtomicU32 = AtomicU32::new(ROOT);

pub fn session_uid() -> Uid {
    SESSION.load(Ordering::Acquire)
}

pub fn set_session_uid(uid: Uid) {
    log::info!("session is now uid {}", uid);
    SESSION.store(uid, Ordering::Release);
}

#[derive(Clone)]
pub struct Account {
    pub name: String,
    pub uid: Uid,
    pub home: String,
    /// The PHC string from `crypto::argon::hash`.
    pub hash: String,
}

/// Parses `/.kos/users`'s text, skipping lines it can't make sense of.
fn parse(text: &str) -> Vec<Account> {
    text.lines().filter_map(|line| {
        let mut fields = line.splitn(4, ':');
        let name = fields.next()?;
        let uid = fields.next()?.parse().ok()?;
        let home = fields.next()?;
        let hash = fields.next()?;
        Some(Account { name: name.into(), uid, home: home.into(), hash: hash.into() })
    }).collect()
}

fn to_text(accounts: &[Account]) -> String {
    accounts.iter().map(|a| format!("{}:{}:{}:{}\n", a.name, a.uid, a.home, a.hash)).collect()
}

/// Every account; none if `/.kos/users` isn't there yet.
pub fn all() -> Vec<Account> {
    match fs_call_as(ROOT, FsRequest::Read { cwd: "/".into(), path: USERS_FILE.into() }) {
        FsResponse::Data(bytes) => parse(&String::from_utf8_lossy(&bytes)),
        _ => Vec::new(),
    }
}

fn save(accounts: &[Account]) -> Result<(), String> {
    let _ = fs_call_as(ROOT, FsRequest::Mkdir { cwd: "/".into(), path: "/.kos".into() });
    match fs_call_as(ROOT, FsRequest::Write { cwd: "/".into(), path: USERS_FILE.into(), data: to_text(accounts).into_bytes() }) {
        FsResponse::Ok => Ok(()),
        FsResponse::Err(e) => Err(format!("can't save {}: {}", USERS_FILE, e)),
        _ => Err(format!("can't save {}", USERS_FILE)),
    }
}

/// Whether any account exists (else the first boot sets KOS up).
pub fn any() -> bool {
    !all().is_empty()
}

pub fn lookup(name: &str) -> Option<Account> {
    all().into_iter().find(|a| a.name == name)
}

/// The name of `uid`'s account, or the number if there isn't one.
pub fn name_of(uid: Uid) -> String {
    match uid {
        NOBODY => "nobody".into(),
        _ => all().into_iter().find(|a| a.uid == uid).map(|a| a.name).unwrap_or_else(|| format!("{}", uid)),
    }
}

/// The uid named `name`: an account's name, or a number.
pub fn uid_of(name: &str) -> Option<Uid> {
    lookup(name).map(|a| a.uid).or_else(|| name.parse().ok())
}

/// Whether `password` is `name`'s. Slow on purpose (Argon2id), and
/// slower still when it's wrong; keep it off the UI task.
pub fn check_password(name: &str, password: &str) -> Option<Account> {
    let account = lookup(name);
    let ok = account.as_ref().is_some_and(|a| crate::crypto::argon::verify(password.as_bytes(), &a.hash));
    if !ok {
        crate::arch::timer::delay_ms(1000);
        return None;
    }
    account
}

/// Lower-case letters, digits, `_` and `-`, starting with a letter, up to
/// 16 of them.
pub fn valid_name(name: &str) -> bool {
    let b = name.as_bytes();
    (1..=16).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_' || *c == b'-')
}

/// Makes an account (root gets uid 0, anyone else the next free one from
/// 1000) with a private home folder: `/root` for root, else
/// `/home/<name>`. Returns it.
pub fn add(name: &str, password: &str) -> Result<Account, String> {
    if !valid_name(name) {
        return Err(String::from("names are 1-16 lower-case letters, digits, _ or -, starting with a letter"));
    }
    let mut accounts = all();
    if accounts.iter().any(|a| a.name == name) {
        return Err(format!("{} already exists", name));
    }
    let (uid, home) = if name == "root" {
        (ROOT, String::from("/root"))
    } else {
        let uid = accounts.iter().map(|a| a.uid + 1).filter(|&u| u >= FIRST_USER).max().unwrap_or(FIRST_USER);
        (uid, format!("/home/{}", name))
    };
    let account = Account { name: name.into(), uid, home: home.clone(), hash: crate::crypto::argon::hash(password.as_bytes()) };
    accounts.push(account.clone());
    save(&accounts)?;
    if uid != ROOT {
        let _ = fs_call_as(ROOT, FsRequest::Mkdir { cwd: "/".into(), path: "/home".into() });
    }
    match fs_call_as(ROOT, FsRequest::Mkdir { cwd: "/".into(), path: home.clone() }) {
        FsResponse::Ok | FsResponse::Err(FsError::AlreadyExists) => {}
        FsResponse::Err(e) => return Err(format!("can't make {}: {}", home, e)),
        _ => {}
    }
    match fs_call_as(ROOT, FsRequest::Chown { cwd: "/".into(), path: home.clone(), uid }) {
        FsResponse::Ok => {}
        FsResponse::Err(e) => return Err(format!("can't give {} to {}: {}", home, name, e)),
        _ => {}
    }
    match fs_call_as(ROOT, FsRequest::Chmod { cwd: "/".into(), path: home.clone(), read: Some(false), write: Some(false) }) {
        FsResponse::Err(e) => return Err(format!("can't make {} private: {}", home, e)),
        _ => {}
    }
    log::info!("added account {} (uid {})", name, uid);
    Ok(account)
}

pub fn set_password(name: &str, password: &str) -> Result<(), String> {
    let mut accounts = all();
    let account = accounts.iter_mut().find(|a| a.name == name).ok_or_else(|| format!("no account named {}", name))?;
    account.hash = crate::crypto::argon::hash(password.as_bytes());
    save(&accounts)
}

/// Removes `name`'s account (not root's). Their files stay.
pub fn remove(name: &str) -> Result<Account, String> {
    if name == "root" {
        return Err(String::from("root's account can't be removed"));
    }
    let mut accounts = all();
    let i = accounts.iter().position(|a| a.name == name).ok_or_else(|| format!("no account named {}", name))?;
    let account = accounts.remove(i);
    save(&accounts)?;
    Ok(account)
}

#[cfg(feature = "test")]
pub mod tests {
    use super::*;

    fn test_users_file_round_trip() {
        let text = "root:0:/root:$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA\nalice:1000:/home/alice:$argon2id$x\nbroken line\n";
        let accounts = parse(text);
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[1].name, "alice");
        assert_eq!(accounts[1].uid, 1000);
        assert_eq!(accounts[0].hash, "$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA");
        assert_eq!(to_text(&accounts), "root:0:/root:$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA\nalice:1000:/home/alice:$argon2id$x\n");
    }

    fn test_valid_names() {
        assert!(valid_name("alice"));
        assert!(valid_name("a_b-9"));
        assert!(!valid_name(""));
        assert!(!valid_name("9lives"));
        assert!(!valid_name("Alice"));
        assert!(!valid_name("a:b"));
        assert!(!valid_name("seventeen-letters"));
    }

    pub fn run() {
        crate::test::run_all(&[
            &test_users_file_round_trip,
            &test_valid_names,
        ]);
    }
}
