//! K-AI's tools: one function per intent, built on the kernel's own
//! functions. Each returns K-AI's answer as text, so the shell and a chat
//! window can both show it. Facts come from live system data, never from
//! the model.
//!
//! Dangerous intents (see `dangerous`) never act straight away: they check
//! what they'd do, then return a question, and only act once you say yes.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use kai::Understanding;
use log::{Level, LevelFilter};
use crate::arch::timer;
use crate::filesys::fs_ipc::{fs_call, FsRequest, FsResponse};
use crate::klog::{self, Snapshot};

/// Known site names, for "open wikipedia". Anything with a dot is taken
/// as an address, and anything else is searched for.
const SITES: &[(&str, &str)] = &[
    ("wikipedia", "https://en.wikipedia.org/"),
    ("google", "https://www.google.com/"),
    ("debian", "https://www.debian.org/"),
    ("github", "https://github.com/"),
    ("wiby", "https://wiby.me/"),
    ("hacker news", "https://news.ycombinator.com/"),
    ("osdev wiki", "https://wiki.osdev.org/"),
    ("osdev", "https://wiki.osdev.org/"),
    ("rust", "https://www.rust-lang.org/"),
    ("the rust website", "https://www.rust-lang.org/"),
    ("duckduckgo", "https://duckduckgo.com/"),
    ("youtube", "https://www.youtube.com/"),
    ("reddit", "https://www.reddit.com/"),
    ("example", "https://example.com/"),
];

/// What K-AI can do, shown for `help` and for an empty request.
pub const HELP: &str = "I can tell you the time, what's running, how much memory is free, how long KOS has been
up and whether the network is connected; ping a host; open a website; list files; delete a
file (I'll always ask first); and read the log to explain what went wrong. Try:
  kai what time is it?       kai what's running?              kai how much memory is free?
  kai am i connected?        kai ping debian.org              kai open wikipedia
  kai what went wrong?       kai why isn't the network working?";

/// Intents that change or destroy something. K-AI only offers these when
/// the model is at least `DANGEROUS_CONFIDENT` sure, and always asks first.
pub fn dangerous(intent: &str) -> bool {
    matches!(intent, "delete_file")
}

/// The confidence a dangerous intent needs before K-AI even asks.
pub const DANGEROUS_CONFIDENT: f32 = 0.9;

/// What a tool did: answered, or wants a yes before going ahead.
pub enum Outcome {
    Done(String),
    Confirm { question: String, action: Pending },
}

/// An action waiting for your yes.
pub enum Pending {
    /// Remove this absolute path.
    Delete { path: String },
}

/// An intent in words, for "did you want me to ...?".
pub fn describe(intent: &str) -> &'static str {
    match intent {
        "help" => "show what I can do",
        "delete_file" => "delete a file",
        "list_tasks" => "show what's running",
        "memory_status" => "check memory",
        "uptime" => "say how long KOS has been up",
        "time" => "tell you the time",
        "network_status" => "check the network",
        "ping" => "ping a host",
        "open_url" => "open a website",
        "list_files" => "list files",
        "diagnose" => "explain what went wrong",
        _ => "do something I can't",
    }
}

fn slot<'a>(u: &'a Understanding, name: &str) -> Option<&'a str> {
    u.slots.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
}

/// Carries out what K-AI understood, or asks first if it's dangerous.
pub fn act(u: &Understanding, text: &str) -> Outcome {
    if u.intent == "delete_file" {
        return match slot(u, "path") {
            Some(path) => prepare_delete(path),
            None => Outcome::Done(String::from("Delete what? Name it, like `kai delete notes.txt`.")),
        };
    }
    Outcome::Done(answer(u, text))
}

/// Carries out an action you said yes to.
pub fn run(action: Pending) -> String {
    match action {
        Pending::Delete { path } => match fs_call(FsRequest::Remove { cwd: String::from("/"), path: path.clone() }) {
            FsResponse::Ok => format!("Deleted {}.", path),
            FsResponse::Err(e) => format!("I couldn't delete {}: {:?}.", path, e),
            _ => String::from("The filesystem gave an unexpected answer."),
        },
    }
}

fn answer(u: &Understanding, text: &str) -> String {
    match u.intent.as_str() {
        "help" => String::from(HELP),
        "list_tasks" => list_tasks(),
        "memory_status" => memory_status(),
        "uptime" => format!("KOS has been up for {}.", duration(timer::uptime_ms())),
        "time" => time(),
        "network_status" => network_status(),
        "ping" => match slot(u, "host") {
            Some(host) => ping(host),
            None => String::from("Ping what? Try `kai ping debian.org`."),
        },
        "open_url" => match slot(u, "site") {
            Some(site) => open_url(site),
            None => String::from("Open what? Try `kai open wikipedia`."),
        },
        "list_files" => list_files(slot(u, "path").unwrap_or(".")),
        "diagnose" => what_went_wrong(slot(u, "topic"), text),
        _ => String::from("Sorry, that's not something I can do yet. Type `kai` to see what I can help with."),
    }
}

/// "2 hours 5 minutes", "3 minutes 12 seconds", "40 seconds".
fn duration(ms: u64) -> String {
    let s = ms / 1000;
    let plural = |n: u64, unit: &str| format!("{} {}{}", n, unit, if n == 1 { "" } else { "s" });
    match (s / 3600, s / 60 % 60, s % 60) {
        (0, 0, sec) => plural(sec, "second"),
        (0, min, sec) => format!("{} {}", plural(min, "minute"), plural(sec, "second")),
        (h, min, _) => format!("{} {}", plural(h, "hour"), plural(min, "minute")),
    }
}

/// "a, b and c".
fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {}", rest.join(", "), last),
    }
}

const MONTH_NAMES: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August",
    "September", "October", "November", "December"];
const DAY_NAMES: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

/// "It's 20:54 on Tuesday, 27 October 2026 (03:54 UTC)."
fn time() -> String {
    use crate::drivers::rtc;
    let (Some(local), Some(unix)) = (rtc::local_now(), rtc::unix_time()) else {
        return format!("KOS has no clock chip, so I don't know the time. It's been up for {}.",
            duration(timer::uptime_ms()));
    };
    let mut answer = format!("It's {:02}:{:02} on {}, {} {} {}", local.hour, local.minute,
        DAY_NAMES[local.weekday as usize], local.day, MONTH_NAMES[local.month as usize - 1], local.year);
    // Mention UTC too when local time isn't UTC.
    if rtc::utc_offset() != 0 {
        let utc = rtc::DateTime::from_unix(unix as i64);
        answer.push_str(&format!(" ({:02}:{:02} UTC)", utc.hour, utc.minute));
    }
    answer.push('.');
    answer
}

fn list_tasks() -> String {
    let mut tasks = crate::scheduler::tasks();
    tasks.sort_by(|a, b| b.cpu_percent.cmp(&a.cpu_percent));
    let busy: Vec<String> = tasks.iter().filter(|t| t.cpu_percent > 0).take(3)
        .map(|t| format!("{} ({}%)", t.name, t.cpu_percent)).collect();
    let names: Vec<String> = tasks.iter().map(|t| t.name.clone()).collect();
    let busiest = if busy.is_empty() {
        String::from("None of them has used a noticeable share of the CPU.")
    } else {
        format!("Busiest since boot: {}.", and_list(&busy))
    };
    format!("There are {} tasks: {}. {} Core 0 has been idle {}% of the time.",
        tasks.len(), and_list(&names), busiest, crate::scheduler::idle_percent())
}

fn memory_status() -> String {
    let (_, free, total) = crate::mm::allocator::ALLOCATOR.heap_remaining();
    let mib = |b: u64| b as f32 / (1024.0 * 1024.0);
    let used = (total - free) * 100 / total.max(1);
    format!("The kernel heap has {:.1} MiB free of {:.1} MiB ({}% used).", mib(free), mib(total), used)
}

fn network_status() -> String {
    let st = crate::net::status();
    if !st.device {
        return String::from("There's no network device, so KOS is offline.");
    }
    let Some(address) = st.address else {
        return String::from("The network card is there, but DHCP hasn't given KOS an address yet.");
    };
    let gateway = st.gateway.map_or(String::from("none"), |g| g.to_string());
    let dns: Vec<String> = st.dns.iter().map(|d| d.to_string()).collect();
    format!("You're connected: address {}, gateway {}, DNS {}.", address, gateway,
        if dns.is_empty() { String::from("none") } else { and_list(&dns) })
}

fn ping(host: &str) -> String {
    match crate::net::ping(host).wait() {
        Ok(ms) => format!("{} replied in {} ms.", host, ms),
        Err(e) => format!("{} didn't reply: {}.", host, e),
    }
}

/// Turns what was said ("wikipedia", "debian.org", a URL) into a URL.
fn site_url(site: &str) -> String {
    let lower = site.to_ascii_lowercase();
    let name = lower.trim_start_matches("the ").trim_end_matches(" website").trim_end_matches(" site");
    if let Some((_, url)) = SITES.iter().find(|(n, _)| *n == lower || *n == name) {
        return String::from(*url);
    }
    if site.contains("://") {
        return String::from(site);
    }
    if site.contains('.') && !site.contains(' ') {
        return format!("https://{}", site);
    }
    // Not a known name or an address: search the small web for it.
    let query: String = site.chars().map(|c| if c == ' ' { '+' } else { c }).collect();
    format!("https://wiby.me/?q={}", query)
}

fn open_url(site: &str) -> String {
    let url = site_url(site);
    let browser = crate::ui::apps::browser::Browser::with_url(Some(&url));
    crate::ui::window::open_app(Box::new(browser));
    format!("Opening {} in the Browser.", url)
}

fn list_files(path: &str) -> String {
    let cwd = crate::shell::fs::current_cwd();
    let request = FsRequest::List { cwd: cwd.clone(), path: String::from(path) };
    // Name the folder the way you'd say it, not as ".".
    let path = if path == "." { format!("The current folder ({})", cwd) } else { String::from(path) };
    let path = path.as_str();
    match fs_call(request) {
        FsResponse::Listing(entries) => {
            let names: Vec<String> = entries.iter()
                .filter(|e| !e.name.starts_with('.'))
                .map(|e| format!("{}{}", e.name, if e.is_dir { "/" } else { "" }))
                .collect();
            match names.len() {
                0 => format!("{} is empty.", path),
                n if n <= 20 => format!("{} has {}: {}.", path, plural_items(n), and_list(&names)),
                n => format!("{} has {}; the first 20: {}.", path, plural_items(n), names[..20].join(", ")),
            }
        }
        FsResponse::Err(e) => format!("I couldn't list {}: {:?}.", path, e),
        _ => String::from("The filesystem gave an unexpected answer."),
    }
}

/// `path` made absolute against `cwd`, with `.` and `..` worked out.
fn absolute(cwd: &str, path: &str) -> String {
    let joined = if path.starts_with('/') { String::from(path) } else { format!("{}/{}", cwd, path) };
    let mut parts: Vec<&str> = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => {}
            ".." => { parts.pop(); }
            p => parts.push(p),
        }
    }
    format!("/{}", parts.join("/"))
}

/// Checks a delete is possible, then asks. Nothing is removed here.
fn prepare_delete(path: &str) -> Outcome {
    let full = absolute(&crate::shell::fs::current_cwd(), path);
    if full == "/" {
        return Outcome::Done(String::from("I won't delete the whole disk."));
    }
    // Look for it in its folder first, so the question is about something
    // real (and a misheard path ends here instead of in a yes/no).
    let (parent, name) = full.rsplit_once('/').unwrap();
    let parent = if parent.is_empty() { "/" } else { parent };
    let entries = match fs_call(FsRequest::List { cwd: String::from("/"), path: String::from(parent) }) {
        FsResponse::Listing(entries) => entries,
        _ => return Outcome::Done(format!("There's no folder {}, so there's nothing to delete.", parent)),
    };
    // FAT names ignore case.
    let Some(entry) = entries.iter().find(|e| e.name.eq_ignore_ascii_case(name)) else {
        return Outcome::Done(format!("There's no file or folder called {}.", full));
    };
    let what = if entry.is_dir { "the folder" } else { "the file" };
    Outcome::Confirm {
        question: format!("Delete {} {}? This can't be undone.", what, full),
        action: Pending::Delete { path: full },
    }
}

fn plural_items(n: usize) -> String {
    format!("{} item{}", n, if n == 1 { "" } else { "s" })
}

// ---- what went wrong -------------------------------------------------------

// "What went wrong?": K-AI reads the kernel log and explains the problems
// in it. No model is involved past understanding the question; this is
// plain code, so it says the same thing every time.
//
// 1. Take the warnings and errors from the log's ring buffer: the ones
//    since you last asked, or all of them for a topic like "the network".
// 2. Group repeats. Each message becomes a template, with anything that
//    has a digit in it (counts, addresses, IPs) replaced by `#`, so
//    "sector 12 failed" and "sector 40 failed" are the same problem.
// 3. Rank: errors before warnings, then the most frequent, then the most
//    recent.
// 4. Explain the ones KOS knows about, and check live state where it helps.

/// How many problems the answer lists.
const SHOWN: usize = 5;

/// When you last asked what went wrong (ms since boot; 0 means never).
static LAST_ASKED: AtomicU64 = AtomicU64::new(0);

/// What people call a part of KOS, and the log targets it covers.
const TOPICS: &[(&[&str], &[&str])] = &[
    (&["tls", "https", "ssl", "cert", "secure"], &["net::tls", "embedded_tls"]),
    (&["net", "internet", "wifi", "wi-fi", "connect", "dhcp", "dns", "ping", "ethernet"],
        &["net", "smoltcp", "drivers::virtio::net"]),
    (&["web", "browser", "site", "page", "http", "download"], &["net", "embedded_tls", "ui::apps::browser"]),
    (&["disk", "file", "folder", "fat", "storage", "save"], &["filesys", "drivers::virtio::blk"]),
    (&["mem", "ram", "heap"], &["mm"]),
    (&["screen", "display", "graphic", "gpu", "window", "desktop"], &["drivers::virtio::gpu", "drivers::display", "drivers::ramfb", "ui"]),
    (&["mouse", "keyboard", "input", "typing"], &["drivers::input", "drivers::virtio::input"]),
    (&["program", "app", "process", "elf"], &["loader", "process", "syscall", "arch::exceptions"]),
    (&["random", "rand"], &["rand"]),
];

/// Known problems: (target prefix, words in the message, what it means).
const KNOWN: &[(&str, &str, &str)] = &[
    ("net::tls", "can't read", "TLS has no root certificates, so no https site can be trusted. `make disk` puts them in /.kos/certs."),
    ("net::tls", "is damaged", "The root certificate bundle in /.kos/certs is corrupt. Rebuild it with `make disk`."),
    ("net::tls", "certificate", "The site's certificate couldn't be verified: its root CA may be missing from /.kos/certs, or the site uses a cipher KOS doesn't support."),
    ("net::tls", "read failed", "The secure connection broke while the page was downloading."),
    ("net", "timed out", "A web request got no answer in time. The site may be slow or down; `kai ping <host>` checks."),
    ("net", "lost dhcp", "KOS lost its network address. It usually gets one back by itself."),
    ("drivers::virtio", "legacy", "QEMU attached an old-style virtio device. Add disable-legacy=on to its -device line."),
    ("drivers::virtio::blk", "read-only", "The disk is attached read-only, so nothing can be saved."),
    ("drivers::virtio::blk", "failed", "The disk failed a request, so a file may not have been read or saved."),
    ("drivers::virtio::blk", "timed out", "The disk stopped answering, so a file may not have been read or saved."),
    ("filesys", "no block device", "KOS found no disk, so there are no files. Run with disk.img attached."),
    ("filesys", "no fat boot signature", "The disk isn't FAT formatted. Recreate it with `make disk`."),
    ("mm", "out of heap", "The kernel ran out of heap memory. Something may be leaking."),
    ("mm", "out of page frames", "KOS ran out of physical memory."),
    ("", "program fault", "A program crashed: it touched memory it shouldn't have, so KOS stopped it."),
    ("", "kernel fault", "The kernel itself crashed. That's a bug in KOS."),
    ("syscall", "unknown", "A program asked the kernel for something it doesn't have, often because it was built for a different version of KOS."),
    ("drivers::virtio::gpu", "", "The graphics device had trouble, so the screen may not update properly."),
    ("rand", "rndr", "This CPU has no hardware random numbers, so KOS uses a weaker source."),
];

/// One problem: every record with the same target and template.
struct Problem<'a> {
    target: &'a str,
    template: String,
    level: Level,
    count: usize,
    last_ms: u64,
    /// The most recent message, as logged.
    example: &'a str,
}

/// `target` is `prefix` or inside it (`net` covers `net::tls`).
fn under(target: &str, prefix: &str) -> bool {
    target.strip_prefix(prefix).is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
}

/// "sector 12 of 0x4000 failed" -> "sector # of # failed".
fn template(msg: &str) -> String {
    let words: Vec<&str> = msg.split(' ')
        .map(|w| if w.bytes().any(|b| b.is_ascii_digit()) { "#" } else { w })
        .collect();
    words.join(" ")
}

fn explain(p: &Problem) -> Option<&'static str> {
    let msg = p.example.to_ascii_lowercase();
    KNOWN.iter()
        .find(|(prefix, words, _)| (prefix.is_empty() || under(p.target, prefix)) && msg.contains(words))
        .map(|(_, _, why)| *why)
}

/// "40s ago", "3 min ago", "2 h ago".
fn ago(ms: u64) -> String {
    let s = timer::uptime_ms().saturating_sub(ms) / 1000;
    match s {
        0..60 => format!("{}s ago", s),
        60..3600 => format!("{} min ago", s / 60),
        _ => format!("{} h ago", s / 3600),
    }
}

/// The first word in `text` that names a part of KOS, and the log targets
/// that part covers: "the networking" -> ("networking", net...). A word
/// counts if it starts with one of the topic's words, so "program" isn't
/// "ram".
fn known_topic(text: &str) -> Option<(&str, &'static [&'static str])> {
    text.split(|c: char| !c.is_ascii_alphanumeric() && c != '-').filter(|w| !w.is_empty()).find_map(|word| {
        let lower = word.to_ascii_lowercase();
        TOPICS.iter().find(|(keys, _)| keys.iter().any(|k| lower.starts_with(k))).map(|(_, targets)| (word, *targets))
    })
}

/// A topic without its "the" or "my"; empty if that's all it was (the
/// model sometimes tags just "the" in "the clock").
fn bare(topic: &str) -> &str {
    let mut t = topic.trim();
    for article in ["the", "my", "a", "an", "this", "that", "it"] {
        if t.eq_ignore_ascii_case(article) {
            return "";
        }
        if t.len() > article.len() && t.as_bytes()[article.len()] == b' ' && t[..article.len()].eq_ignore_ascii_case(article) {
            t = t[article.len()..].trim_start();
        }
    }
    t
}

/// Groups records into problems, worst first.
fn problems<'a>(records: &[&'a Snapshot]) -> Vec<Problem<'a>> {
    let mut out: Vec<Problem> = Vec::new();
    for r in records {
        let t = template(&r.msg);
        match out.iter_mut().find(|p| p.target == r.target && p.template == t) {
            Some(p) => {
                p.count += 1;
                p.level = p.level.min(r.level);
                p.last_ms = r.time_ms;
                p.example = &r.msg;
            }
            None => out.push(Problem { target: &r.target, template: t, level: r.level, count: 1, last_ms: r.time_ms, example: &r.msg }),
        }
    }
    out.sort_by(|a, b| a.level.cmp(&b.level).then(b.count.cmp(&a.count)).then(b.last_ms.cmp(&a.last_ms)));
    out
}

/// K-AI's answer to "what went wrong (with \<topic\>)?". `text` is what was
/// typed: when the model found no topic, a part of KOS named anywhere in
/// it is the topic (the model misses topics in words it hasn't seen).
fn what_went_wrong(topic: Option<&str>, text: &str) -> String {
    let all = klog::records(Level::Warn);
    let topic = topic.map(str::trim).filter(|t| !bare(t).is_empty()).or_else(|| known_topic(text).map(|(word, _)| word));
    let targets = topic.and_then(known_topic).map(|(_, targets)| targets);

    let (records, scope): (Vec<&Snapshot>, String) = match topic {
        // About a topic: everything still in the log, from the parts of
        // KOS it covers (or, for one KOS doesn't know, that mention it).
        Some(topic) => {
            let lower = bare(topic).to_ascii_lowercase();
            let lower = lower.as_str();
            let records = match targets {
                Some(targets) => all.iter().filter(|r| targets.iter().any(|t| under(&r.target, t))).collect(),
                None => all.iter().filter(|r| r.target.contains(lower) || r.msg.to_ascii_lowercase().contains(lower)).collect(),
            };
            (records, format!("with {}", topic))
        }
        // In general: what's new since you last asked.
        None => {
            let since = LAST_ASKED.swap(timer::uptime_ms(), Ordering::Relaxed);
            let scope = if since == 0 { String::from("since KOS started") } else { format!("since you last asked, {}", ago(since)) };
            (all.iter().filter(|r| r.time_ms > since).collect(), scope)
        }
    };

    let problems = problems(&records);
    let mut answer = match problems.len() {
        0 => format!("Nothing has gone wrong {}.", scope),
        1 => format!("I found 1 problem {}:", scope),
        n => format!("I found {} problems {}:", n, scope),
    };
    for (i, p) in problems.iter().take(SHOWN).enumerate() {
        let level = if p.level == Level::Error { "error" } else { "warning" };
        let times = if p.count == 1 { String::new() } else { format!("{} times, ", p.count) };
        answer.push_str(&format!("\n  {}. {} in {} ({}last {}): {}", i + 1, level, p.target, times, ago(p.last_ms), p.example));
        if let Some(why) = explain(p) {
            answer.push_str(&format!("\n     {}", why));
        }
    }
    if problems.len() > SHOWN {
        answer.push_str(&format!("\n  ...and {} more. `dmesg` shows the whole log.", problems.len() - SHOWN));
    }

    // Live state, for the parts of KOS that have it.
    let net = targets.is_some_and(|t| t.contains(&"net"))
        || problems.iter().take(SHOWN).any(|p| under(p.target, "net"));
    if net {
        answer.push_str(&format!("\nRight now: {}", network_status()));
    }
    if targets.is_some_and(|t| t.contains(&"mm")) {
        answer.push_str(&format!("\nRight now: {}", memory_status()));
    }

    // Only warnings and errors are kept by default. If the topic's part of
    // KOS isn't logging its details, say how to turn them on.
    if let Some(target) = targets.map(|t| t[0]) {
        if klog::level_for(target) < LevelFilter::Debug {
            answer.push_str(&format!("\nFor more detail, run `log {} debug`, make it happen again, and ask me again.", target));
        }
    }
    answer
}
