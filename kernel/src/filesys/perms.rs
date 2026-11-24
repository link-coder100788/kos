//! Who may read and write which files. Simpler than Unix: no groups, no
//! execute bit. Every file and folder has an owner and two flags for
//! everyone else: `r`, others may read it (a folder: list it and look
//! inside), and `w`, others may write it (a folder: create and remove
//! things in it). The owner can always do both, and root can do anything.
//!
//! Only paths that differ from their parent folder have an entry: a path
//! without one has its parent's owner and flags, and `/` defaults to root
//! with `r` set. So `/home/alice` owned by alice with no flags makes
//! everything inside it alice's and private, with one entry. Something
//! created in a folder its creator doesn't own gets its own entry (owner:
//! the creator, `r` as the folder's, `w` clear).
//!
//! Reading needs every folder on the way to be readable, so a private
//! folder hides all of it. Writing, creating and removing need that too,
//! plus `w` (or ownership) on the thing itself; removing also needs it on
//! the folder it's in.
//!
//! The entries are kept in `/.kos/perms`, a line each: `uid flags path`
//! (flags `rw`, `r-`, `-w` or `--`). Paths are absolute, with `.` and `..`
//! resolved, and lowercased: FAT ignores case, so `/HOME/Alice` is
//! `/home/alice`. fs_task owns the one `Perms` and checks every request
//! against it; see `fs_task::handle`.

use alloc::string::String;
use alloc::vec::Vec;
use crate::users::{Uid, ROOT};

/// Where the entries are kept.
pub const PERMS_FILE: &str = "/.kos/perms";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Attrs {
    pub owner: Uid,
    /// Others may read.
    pub read: bool,
    /// Others may write.
    pub write: bool,
}

/// What `/` has without an entry: root's, readable by everyone.
const DEFAULT: Attrs = Attrs { owner: ROOT, read: true, write: false };

pub struct Perms {
    /// (normalized path, attributes), in no particular order.
    entries: Vec<(String, Attrs)>,
}

/// `path` relative to `cwd`, made absolute, with `.` and `..` resolved
/// and lowercased: `normalize("/Home", "a/../B/")` is `/home/b`.
pub fn normalize(cwd: &str, path: &str) -> String {
    let joined = super::backend::join_path(cwd, path);
    let mut parts: Vec<&str> = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => {}
            ".." => { parts.pop(); }
            p => parts.push(p),
        }
    }
    let mut out = String::new();
    for part in parts {
        out.push('/');
        out.push_str(part);
    }
    if out.is_empty() {
        out.push('/');
    }
    out.make_ascii_lowercase();
    out
}

/// The folder `path` (normalized) is in; `/` for `/`.
pub fn parent(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) | None => "/",
        Some(i) => &path[..i],
    }
}

/// `/`, then each folder down to `path` (normalized), then `path`.
fn prefixes(path: &str) -> impl Iterator<Item = &str> {
    let ends = path.match_indices('/').map(|(i, _)| i).skip(1).chain(core::iter::once(path.len()));
    core::iter::once("/").chain(ends.filter(move |_| path != "/").map(move |end| &path[..end]))
}

/// `rw`, `r-`, `-w` or `--`.
pub fn flags_text(attrs: Attrs) -> &'static str {
    match (attrs.read, attrs.write) {
        (true, true) => "rw",
        (true, false) => "r-",
        (false, true) => "-w",
        (false, false) => "--",
    }
}

impl Perms {
    pub const fn new() -> Self {
        Self { entries: Vec::new() }
    }

    /// Reads `/.kos/perms`'s text, skipping lines it can't make sense of.
    pub fn parse(text: &str) -> Self {
        let entries = text.lines().filter_map(|line| {
            let mut fields = line.splitn(3, ' ');
            let owner = fields.next()?.parse().ok()?;
            let flags = fields.next()?.as_bytes();
            let path = fields.next()?;
            if flags.len() != 2 || !path.starts_with('/') {
                return None;
            }
            let attrs = Attrs { owner, read: flags[0] == b'r', write: flags[1] == b'w' };
            Some((normalize("/", path), attrs))
        }).collect();
        Self { entries }
    }

    /// The text for `/.kos/perms`, sorted by path.
    pub fn to_text(&self) -> String {
        use core::fmt::Write;
        let mut entries: Vec<&(String, Attrs)> = self.entries.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let mut text = String::new();
        for (path, attrs) in entries {
            let _ = writeln!(text, "{} {} {}", attrs.owner, flags_text(*attrs), path);
        }
        text
    }

    fn entry(&self, path: &str) -> Option<Attrs> {
        self.entries.iter().find(|(p, _)| p == path).map(|(_, a)| *a)
    }

    /// Whether `path` (normalized) has an entry of its own.
    pub fn has_entry(&self, path: &str) -> bool {
        self.entry(path).is_some()
    }

    /// The owner and flags `path` (normalized) has, its own or inherited.
    pub fn attrs(&self, path: &str) -> Attrs {
        prefixes(path).fold(DEFAULT, |inherited, p| self.entry(p).unwrap_or(inherited))
    }

    /// Whether `uid` may look inside every folder `path` is in.
    fn may_reach(&self, uid: Uid, path: &str) -> bool {
        let mut attrs = DEFAULT;
        for p in prefixes(path) {
            if p == path {
                return true;
            }
            attrs = self.entry(p).unwrap_or(attrs);
            if attrs.owner != uid && !attrs.read {
                return false;
            }
        }
        true
    }

    /// Whether `uid` may read `path` (or list it, or `cd` into it).
    pub fn may_read(&self, uid: Uid, path: &str) -> bool {
        let attrs = self.attrs(path);
        uid == ROOT || (self.may_reach(uid, path) && (attrs.owner == uid || attrs.read))
    }

    /// Whether `uid` may write `path`, or create it if it isn't there.
    pub fn may_write(&self, uid: Uid, path: &str) -> bool {
        let attrs = self.attrs(path);
        uid == ROOT || (self.may_reach(uid, path) && (attrs.owner == uid || attrs.write))
    }

    /// Whether `uid` may remove `path`: write it, and the folder it's in.
    pub fn may_remove(&self, uid: Uid, path: &str) -> bool {
        path != "/" && self.may_write(uid, path) && self.may_write(uid, parent(path))
    }

    /// Whether `uid` may change `path`'s flags: root, or its owner.
    pub fn may_chmod(&self, uid: Uid, path: &str) -> bool {
        uid == ROOT || (self.may_reach(uid, path) && self.attrs(path).owner == uid)
    }

    /// Gives `path` its own entry, or replaces the one it has. An entry
    /// the same as what it would inherit is dropped instead.
    pub fn set(&mut self, path: &str, attrs: Attrs) {
        self.entries.retain(|(p, _)| p != path);
        if path == "/" || self.attrs(parent(path)) != attrs {
            self.entries.push((String::from(path), attrs));
        }
    }

    /// `uid` just created `path`: if the folder it's in isn't theirs, it
    /// gets an entry making it theirs, readable if the folder is, not
    /// writable by others. Returns whether anything changed.
    pub fn created(&mut self, uid: Uid, path: &str) -> bool {
        let folder = self.attrs(parent(path));
        if self.has_entry(path) || folder.owner == uid {
            return false;
        }
        self.set(path, Attrs { owner: uid, read: folder.read, write: false });
        true
    }

    /// `path` was removed: drops its entry and every entry inside it.
    /// Returns whether anything changed.
    /// `from` (normalized) was moved to `to`: its entries, and those of
    /// anything in it, move too, and it keeps the owner and flags it had
    /// rather than taking its new folder's. Returns whether anything
    /// changed.
    pub fn renamed(&mut self, from: &str, to: &str) -> bool {
        let attrs = self.attrs(from);
        let mut changed = false;
        for (p, _) in self.entries.iter_mut() {
            if p == from || (p.starts_with(from) && p.as_bytes().get(from.len()) == Some(&b'/')) {
                *p = alloc::format!("{}{}", to, &p[from.len()..]);
                changed = true;
            }
        }
        if self.attrs(to) != attrs {
            self.set(to, attrs);
            changed = true;
        }
        changed
    }

    pub fn removed(&mut self, path: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|(p, _)| {
            !(p == path || (p.starts_with(path) && p.as_bytes().get(path.len()) == Some(&b'/')))
        });
        self.entries.len() != before
    }
}

#[cfg(feature = "test")]
pub mod tests {
    use super::*;

    const ALICE: Uid = 1000;
    const BOB: Uid = 1001;

    fn sample() -> Perms {
        Perms::parse("1000 -- /home/alice\n1001 r- /home/bob\n0 rw /tmp\n0 -- /.kos/users\n")
    }

    fn test_normalize_resolves_dots_and_case() {
        assert_eq!(normalize("/", "/"), "/");
        assert_eq!(normalize("/Home", "a/../B/"), "/home/b");
        assert_eq!(normalize("/home/alice", ".."), "/home");
        assert_eq!(normalize("/", "../../x//y/."), "/x/y");
        assert_eq!(normalize("/a", "/.KOS/Users"), "/.kos/users");
    }

    fn test_prefixes_walk_down_from_the_root() {
        let all: Vec<&str> = prefixes("/home/alice/notes.txt").collect();
        assert_eq!(all, ["/", "/home", "/home/alice", "/home/alice/notes.txt"]);
        assert_eq!(prefixes("/").collect::<Vec<_>>(), ["/"]);
    }

    fn test_attrs_are_inherited() {
        let p = sample();
        assert_eq!(p.attrs("/bin/ls.elf.b64"), DEFAULT);
        assert_eq!(p.attrs("/home/alice/a/b.txt"), Attrs { owner: ALICE, read: false, write: false });
    }

    fn test_private_folders_hide_their_contents() {
        let p = sample();
        assert!(p.may_read(ALICE, "/home/alice/a/b.txt"));
        assert!(!p.may_read(BOB, "/home/alice/a/b.txt"));
        assert!(!p.may_read(BOB, "/home/alice"));
        assert!(p.may_read(BOB, "/home"));
        assert!(p.may_read(ROOT, "/home/alice/a/b.txt"));
        assert!(!p.may_read(ALICE, "/.kos/users"));
        assert!(p.may_read(ALICE, "/.kos/certs/roots.der"));
    }

    fn test_writing_needs_ownership_or_w() {
        let p = sample();
        assert!(p.may_write(ALICE, "/home/alice/new.txt"));
        assert!(!p.may_write(ALICE, "/home/bob/new.txt"));
        assert!(p.may_read(ALICE, "/home/bob/notes.txt"));
        assert!(!p.may_write(ALICE, "/bin/ls.elf.b64"));
        assert!(p.may_write(ALICE, "/tmp/x"));
        assert!(p.may_write(ROOT, "/bin/ls.elf.b64"));
        assert!(!p.may_remove(ALICE, "/home/alice"));
        assert!(p.may_remove(ALICE, "/home/alice/a"));
        assert!(!p.may_remove(ROOT, "/"));
    }

    fn test_created_files_belong_to_their_creator() {
        let mut p = sample();
        assert!(!p.created(ALICE, "/home/alice/x"));
        assert!(p.created(BOB, "/tmp/bobs"));
        assert_eq!(p.attrs("/tmp/bobs"), Attrs { owner: BOB, read: true, write: false });
        assert!(!p.may_write(ALICE, "/tmp/bobs"));
        assert!(!p.may_remove(ALICE, "/tmp/bobs"));
        assert!(p.may_write(BOB, "/tmp/bobs/inner"));
    }

    fn test_removing_drops_the_whole_subtree() {
        let mut p = sample();
        p.set("/home/alice/pub", Attrs { owner: ALICE, read: true, write: false });
        p.set("/home/alicex", Attrs { owner: BOB, read: true, write: false });
        assert!(p.removed("/home/alice"));
        assert!(!p.has_entry("/home/alice/pub"));
        assert!(p.has_entry("/home/alicex"));
        assert!(!p.removed("/home/alice"));
    }

    fn test_set_drops_entries_that_match_the_parent() {
        let mut p = sample();
        p.set("/home/alice/a", Attrs { owner: ALICE, read: false, write: false });
        assert!(!p.has_entry("/home/alice/a"));
        assert!(p.may_chmod(ALICE, "/home/alice/a"));
        assert!(!p.may_chmod(BOB, "/home/alice/a"));
    }

    fn test_text_round_trip() {
        let p = sample();
        let again = Perms::parse(&p.to_text());
        assert_eq!(again.to_text(), p.to_text());
        assert_eq!(again.attrs("/home/bob"), Attrs { owner: BOB, read: true, write: false });
    }

    fn test_renaming_moves_entries_and_keeps_attrs() {
        let mut p = sample();
        p.set("/home/alice/notes/x.txt", Attrs { owner: ALICE, read: true, write: false });
        // Out of alice's private folder into /tmp: still hers and private.
        assert!(p.renamed("/home/alice/notes", "/tmp/notes"));
        assert_eq!(p.attrs("/tmp/notes"), Attrs { owner: ALICE, read: false, write: false });
        assert_eq!(p.attrs("/tmp/notes/x.txt"), Attrs { owner: ALICE, read: true, write: false });
        assert!(!p.has_entry("/home/alice/notes/x.txt"));
        // Within a folder, nothing to change.
        assert!(!p.renamed("/home/bob/a", "/home/bob/b"));
    }

    pub fn run() {
        crate::test::run_all(&[
            &test_renaming_moves_entries_and_keeps_attrs,
            &test_normalize_resolves_dots_and_case,
            &test_prefixes_walk_down_from_the_root,
            &test_attrs_are_inherited,
            &test_private_folders_hide_their_contents,
            &test_writing_needs_ownership_or_w,
            &test_created_files_belong_to_their_creator,
            &test_removing_drops_the_whole_subtree,
            &test_set_drops_entries_that_match_the_parent,
            &test_text_round_trip,
        ]);
    }
}
