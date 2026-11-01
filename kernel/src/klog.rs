//! Kernel logging.
//!
//! Use the `log` crate's macros anywhere (`log::debug!("...")`); the target
//! is the module path, shortened to e.g. `net::http` or `scheduler`.
//!
//! - Filters decide what gets logged at all: a default level, plus
//!   per-target overrides that apply to a module and everything inside it
//!   (`net` covers `net::http`). The longest matching rule wins.
//! - Everything logged goes into a ring buffer of the last `RING` records
//!   (`dmesg`, and dumped automatically on a panic).
//! - Records at or above the console level are also printed.
//!
//! Set filters at boot with `make run LOG="net=trace,console=debug"` (passed
//! in through QEMU's fw_cfg as `opt/kos/log`), or live with the shell's
//! `log` command. Nothing here allocates, so logging is safe from IRQ
//! handlers, the allocator and the panic handler.

use core::fmt::{self, Write};
use log::{Level, LevelFilter, Log, Metadata, Record};
use crate::std::lock::irq_mutex::IrqMutex;

const RING: usize = 1024;
const MSG_LEN: usize = 200;
const TARGET_LEN: usize = 24;
const MAX_RULES: usize = 16;

// ---- filters --------------------------------------------------------------

#[derive(Clone, Copy)]
struct Rule {
    target: [u8; TARGET_LEN],
    len: usize,
    level: LevelFilter,
}

struct Filters {
    default: LevelFilter,
    console: LevelFilter,
    rules: [Option<Rule>; MAX_RULES],
}

const DEFAULT_FILTERS: Filters = Filters {
    default: LevelFilter::Info,
    console: LevelFilter::Info,
    rules: [
        // smoltcp traces every packet; keep it to warnings unless asked.
        Some(Rule { target: *b"smoltcp\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0", len: 7, level: LevelFilter::Warn }),
        // embedded-tls warns about extensions it skips, which servers send
        // on purpose (GREASE) to check clients skip them.
        Some(Rule { target: *b"embedded_tls\0\0\0\0\0\0\0\0\0\0\0\0", len: 12, level: LevelFilter::Error }),
        None, None, None, None, None, None, None, None, None, None, None, None, None, None,
    ],
};

static FILTERS: IrqMutex<Filters> = IrqMutex::new(DEFAULT_FILTERS);

/// `kos_kernel::net::http` -> `net::http`.
fn short_target(target: &str) -> &str {
    target.strip_prefix("kos_kernel::").unwrap_or(target)
}

fn rule_matches(rule: &Rule, target: &str) -> bool {
    let name = &rule.target[..rule.len];
    let t = target.as_bytes();
    t.starts_with(name) && (t.len() == name.len() || t[name.len()..].starts_with(b"::"))
}

impl Filters {
    fn level_for(&self, target: &str) -> LevelFilter {
        let mut best: Option<&Rule> = None;
        for rule in self.rules.iter().flatten() {
            if rule_matches(rule, target) && best.is_none_or(|b| rule.len > b.len) {
                best = Some(rule);
            }
        }
        best.map_or(self.default, |r| r.level)
    }

    /// The most verbose level anything is set to, for `log::set_max_level`.
    fn max(&self) -> LevelFilter {
        self.rules.iter().flatten().map(|r| r.level).fold(self.default, |a, b| a.max(b))
    }
}

/// Parses a level name. Doesn't allocate: the boot spec is applied before
/// the heap exists.
pub fn parse_level(s: &str) -> Option<LevelFilter> {
    const NAMES: [(&str, &str, LevelFilter); 6] = [
        ("off", "o", LevelFilter::Off),
        ("error", "e", LevelFilter::Error),
        ("warn", "w", LevelFilter::Warn),
        ("info", "i", LevelFilter::Info),
        ("debug", "d", LevelFilter::Debug),
        ("trace", "t", LevelFilter::Trace),
    ];
    NAMES.iter().find(|(long, short, _)| s.eq_ignore_ascii_case(long) || s.eq_ignore_ascii_case(short)).map(|(_, _, l)| *l)
}

/// Sets the level for `target`: "default" / "all", "console", or a module
/// path like "net" or "drivers::virtio".
pub fn set_level(target: &str, level: LevelFilter) -> Result<(), &'static str> {
    let mut f = FILTERS.lock();
    match target {
        "default" | "all" | "*" => f.default = level,
        "console" => f.console = level,
        _ => {
            let target = short_target(target);
            if target.is_empty() || target.len() > TARGET_LEN {
                return Err("target name too long");
            }
            let mut name = [0u8; TARGET_LEN];
            name[..target.len()].copy_from_slice(target.as_bytes());
            let rule = Rule { target: name, len: target.len(), level };
            let slot = f.rules.iter().position(|r| r.is_some_and(|r| &r.target[..r.len] == target.as_bytes()))
                .or_else(|| f.rules.iter().position(|r| r.is_none()))
                .ok_or("too many log rules")?;
            f.rules[slot] = Some(rule);
        }
    }
    log::set_max_level(f.max());
    Ok(())
}

/// Applies a spec like `debug,net=trace,console=debug,smoltcp=info`.
pub fn apply_spec(spec: &str) -> Result<(), &'static str> {
    for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (target, level) = part.split_once('=').unwrap_or(("default", part));
        let level = parse_level(level.trim()).ok_or("unknown level (use off/error/warn/info/debug/trace)")?;
        set_level(target.trim(), level)?;
    }
    Ok(())
}

pub fn reset() {
    let mut f = FILTERS.lock();
    *f = DEFAULT_FILTERS;
    log::set_max_level(f.max());
}

/// The level `target` is logged at, after the rules.
pub fn level_for(target: &str) -> LevelFilter {
    FILTERS.lock().level_for(short_target(target))
}

/// Prints the current filters.
pub fn print_filters() {
    let (default, console, rules) = {
        let f = FILTERS.lock();
        (f.default, f.console, f.rules)
    };
    crate::println!("console  {}  (records at this level or above are printed)", console);
    crate::println!("default  {}", default);
    for rule in rules.iter().flatten() {
        let name = core::str::from_utf8(&rule.target[..rule.len]).unwrap_or("?");
        crate::println!("{:<8} {}", name, rule.level);
    }
}

// ---- records and the ring buffer --------------------------------------------

#[derive(Clone, Copy)]
struct Entry {
    time_ms: u64,
    /// `Level as u8` (1 = error .. 5 = trace).
    level: u8,
    core: u8,
    in_irq: bool,
    task: u64,
    task_name: [u8; 12],
    target: [u8; TARGET_LEN],
    target_len: u8,
    msg: [u8; MSG_LEN],
    msg_len: u8,
}

const EMPTY: Entry = Entry {
    time_ms: 0,
    level: 0,
    core: 0,
    in_irq: false,
    task: 0,
    task_name: [0; 12],
    target: [0; TARGET_LEN],
    target_len: 0,
    msg: [0; MSG_LEN],
    msg_len: 0,
};

struct Ring {
    entries: [Entry; RING],
    next: usize,
    count: usize,
}

static RING_BUF: IrqMutex<Ring> = IrqMutex::new(Ring { entries: [EMPTY; RING], next: 0, count: 0 });

/// Formats into a fixed buffer, truncating with "..." if it doesn't fit.
struct FixedWriter<'a> {
    buf: &'a mut [u8],
    len: usize,
    truncated: bool,
}

impl Write for FixedWriter<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for &b in s.as_bytes() {
            if self.len < self.buf.len() {
                self.buf[self.len] = if b == b'\n' { b' ' } else { b };
                self.len += 1;
            } else {
                self.truncated = true;
            }
        }
        Ok(())
    }
}

impl Entry {
    fn target(&self) -> &str {
        core::str::from_utf8(&self.target[..self.target_len as usize]).unwrap_or("?")
    }

    fn msg(&self) -> &str {
        core::str::from_utf8(&self.msg[..self.msg_len as usize]).unwrap_or("?")
    }

    fn level(&self) -> &'static str {
        match self.level {
            1 => "ERROR",
            2 => "WARN",
            3 => "INFO",
            4 => "DEBUG",
            _ => "TRACE",
        }
    }

    fn print(&self) {
        let secs = self.time_ms / 1000;
        let ms = self.time_ms % 1000;
        let name_len = self.task_name.iter().position(|&b| b == 0).unwrap_or(self.task_name.len());
        let name = core::str::from_utf8(&self.task_name[..name_len]).unwrap_or("?");
        let ctx: &str = if self.in_irq { "irq" } else if self.task != u64::MAX { name } else { "-" };
        crate::println!(
            "[{:>5}.{:03} c{} {:<9}] {:<5} {}: {}",
            secs, ms, self.core, ctx, self.level(), self.target(), self.msg()
        );
    }
}

// ---- IRQ context tracking ---------------------------------------------------

static IN_IRQ: [core::sync::atomic::AtomicBool; 8] = [const { core::sync::atomic::AtomicBool::new(false) }; 8];

/// Marks this core as running an interrupt handler (for the log context).
pub fn set_in_irq(on: bool) {
    let core = crate::arch::psci::get_core_id() as usize;
    if core < IN_IRQ.len() {
        IN_IRQ[core].store(on, core::sync::atomic::Ordering::Relaxed);
    }
}

// ---- the logger -------------------------------------------------------------

pub struct KernelLog;

pub static LOGGER: KernelLog = KernelLog;

impl Log for KernelLog {
    fn enabled(&self, metadata: &Metadata) -> bool {
        // try_lock: never wait (or deadlock) just to decide whether to log.
        match FILTERS.try_lock() {
            Some(f) => metadata.level() <= f.level_for(short_target(metadata.target())),
            None => metadata.level() <= Level::Warn,
        }
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let core = crate::arch::psci::get_core_id() as u8;
        let mut entry = EMPTY;
        entry.time_ms = crate::arch::timer::uptime_ms();
        entry.level = record.level() as u8;
        entry.core = core;
        entry.in_irq = IN_IRQ.get(core as usize).is_some_and(|f| f.load(core::sync::atomic::Ordering::Relaxed));
        entry.task = match crate::scheduler::current_task_label(&mut entry.task_name) {
            Some(id) => id,
            None => u64::MAX,
        };
        let target = short_target(record.target());
        let tlen = target.len().min(TARGET_LEN);
        entry.target[..tlen].copy_from_slice(&target.as_bytes()[..tlen]);
        entry.target_len = tlen as u8;
        let mut w = FixedWriter { buf: &mut entry.msg, len: 0, truncated: false };
        let _ = w.write_fmt(*record.args());
        if w.truncated {
            let n = w.len;
            w.buf[n - 3..n].copy_from_slice(b"...");
        }
        entry.msg_len = w.len as u8;

        if let Some(mut ring) = RING_BUF.try_lock() {
            let i = ring.next;
            ring.entries[i] = entry;
            ring.next = (i + 1) % RING;
            ring.count = (ring.count + 1).min(RING);
        }
        let console = FILTERS.try_lock().map_or(LevelFilter::Warn, |f| f.console);
        if record.level() <= console {
            entry.print();
        }
    }

    fn flush(&self) {}
}

/// Installs the logger and applies the boot-time spec from fw_cfg, if any.
pub fn init() {
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(FILTERS.lock().max());
    let mut spec = [0u8; 256];
    if let Some(len) = crate::drivers::fw_cfg::read_file("opt/kos/log", &mut spec) {
        let text = core::str::from_utf8(&spec[..len]).unwrap_or("").trim_end_matches('\0').trim();
        match apply_spec(text) {
            Ok(()) => log::info!("log filters from boot: {}", text),
            Err(e) => log::warn!("bad LOG spec '{}': {}", text, e),
        }
    }
}

/// Prints the last `n` records, optionally only those containing `filter`
/// in their target or message.
pub fn dump(n: usize, filter: Option<&str>) {
    let mut out = alloc::vec::Vec::with_capacity(RING);
    {
        let Some(ring) = RING_BUF.try_lock() else { return };
        let start = (ring.next + RING - ring.count) % RING;
        for i in 0..ring.count {
            out.push(ring.entries[(start + i) % RING]);
        }
    }
    let matching: alloc::vec::Vec<&Entry> = out
        .iter()
        .filter(|e| filter.is_none_or(|f| e.target().contains(f) || e.msg().contains(f)))
        .collect();
    for e in matching.iter().skip(matching.len().saturating_sub(n)) {
        e.print();
    }
}

/// For the panic handler: prints the last `n` records without allocating.
/// A buffered record, copied out for K-AI to read.
pub struct Snapshot {
    pub time_ms: u64,
    pub level: Level,
    pub target: alloc::string::String,
    pub msg: alloc::string::String,
}

/// The buffered records at `level` or more severe, oldest first.
pub fn records(level: Level) -> alloc::vec::Vec<Snapshot> {
    let mut out = alloc::vec::Vec::new();
    let Some(ring) = RING_BUF.try_lock() else { return out };
    let start = (ring.next + RING - ring.count) % RING;
    let entries: alloc::vec::Vec<Entry> = (0..ring.count)
        .map(|i| ring.entries[(start + i) % RING])
        .filter(|e| e.level <= level as u8)
        .collect();
    drop(ring);
    for e in &entries {
        let level = match e.level {
            1 => Level::Error,
            2 => Level::Warn,
            3 => Level::Info,
            4 => Level::Debug,
            _ => Level::Trace,
        };
        out.push(Snapshot { time_ms: e.time_ms, level, target: e.target().into(), msg: e.msg().into() });
    }
    out
}

pub fn dump_for_panic(n: usize) {
    let Some(ring) = RING_BUF.try_lock() else { return };
    let shown = n.min(ring.count);
    crate::println!("--- last {} log records ---", shown);
    for i in 0..shown {
        ring.entries[(ring.next + RING - shown + i) % RING].print();
    }
    crate::println!("--- end of log ---");
}
