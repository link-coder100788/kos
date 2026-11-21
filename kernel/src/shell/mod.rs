//! The console shell. `uart_read_task` reads console input (the serial
//! port and the Terminal window) a line at a time and runs it as a
//! command. While a program started with `run` is going, the shell waits
//! for it, and the program reads the console itself (`readln`). Programs
//! can be joined into pipelines and sent to files (see `fs`).
//!
//! Nothing runs until someone logs in (the first boot sets up the root
//! account and a first user instead). The shell, the desktop and K-AI
//! then act as that user; `su` acts as someone else until `exit`, and
//! `logout` ends their programs and windows and asks again.

pub mod fs;

use alloc::borrow::ToOwned;
use alloc::string::String;
use alloc::vec::Vec;
use crate::arch::stacktrace::RegisterDump;
use crate::arch::timer::Instant;
use crate::drivers::uart::Uart;
use crate::mm::allocator::ALLOCATOR;
use crate::println;
use crate::std::thread;
use crate::users::{self, Uid, ROOT};
use crate::util;

enum UartCommand<'a> {
    HeapRemain,
    LastAllocs,
    OverflowHeap(&'a str),
    ShellCommand(fs::ShellCommand),
    TestThreadTask,
    TestAsyncTask,
    TestHash(&'a str),
    DumpRegs,
    AllDump,
    KShellCommand(crate::k::shell::KShellCommand),
    TestCoreRestart,
    HeapTest,
    Net,
    Ping(&'a str),
    Dns(&'a str),
    Fetch(&'a str),
    Browse(&'a str),
    Log(&'a str),
    Dmesg(&'a str),
    Stats,
    Mmu(&'a str),
    Load(&'a str),
    Kai(&'a str),
    Time,
    Beep,
    Snd(&'a str),
    Play(&'a str),
}

impl<'a> UartCommand<'a> {
    fn parse(s: &'a str) -> Option<Self> {
        if s.starts_with("hash") {
            return Some(UartCommand::TestHash(s));
        }
        if s == "log" || s.starts_with("log ") {
            return Some(UartCommand::Log(s[3..].trim()));
        }
        if s == "dmesg" || s.starts_with("dmesg ") {
            return Some(UartCommand::Dmesg(s[5..].trim()));
        }
        if s == "load" || s.starts_with("load ") {
            return Some(UartCommand::Load(s[4..].trim()))
        }
        if s == "kai" || s.starts_with("kai ") {
            return Some(UartCommand::Kai(s[3..].trim()));
        }
        if s == "snd" || s.starts_with("snd ") {
            return Some(UartCommand::Snd(s[3..].trim()));
        }
        if s == "play" || s.starts_with("play ") {
            return Some(UartCommand::Play(s[4..].trim()));
        }
        if let Some(name) = s.strip_prefix("dns ") {
            return Some(UartCommand::Dns(name.trim()));
        }
        if let Some(host) = s.strip_prefix("ping ") {
            return Some(UartCommand::Ping(host.trim()));
        }
        if let Some(url) = s.strip_prefix("browse") {
            return Some(UartCommand::Browse(url.trim()));
        }
        if let Some(url) = s.strip_prefix("fetch ") {
            return Some(UartCommand::Fetch(url.trim()));
        }
        if let Some(arg) = s.strip_prefix("mmu") {
            return Some(UartCommand::Mmu(arg.trim()));
        }
        if s == "k" || s.starts_with("k ") {
            return match crate::k::shell::KShellCommand::parse(s[1..].trim()) {
                Some(command) => Some(UartCommand::KShellCommand(command)),
                None => {
                    println!("k: usage: k build|run|ast|tokenize|modules|semantic|ir|disassemble <file>");
                    None
                }
            };
        }
        match s {
            "heap_remain" => Some(UartCommand::HeapRemain),
            "last_allocs" => Some(UartCommand::LastAllocs),
            "!!_overflow_heap" => Some(UartCommand::OverflowHeap(s)),
            "test_thread_task" => Some(UartCommand::TestThreadTask),
            "ttt" => Some(UartCommand::TestThreadTask),
            "aaa" => Some(UartCommand::TestAsyncTask),
            "dump" => Some(UartCommand::DumpRegs),
            "all" => Some(UartCommand::AllDump),
            "tcr" => Some(UartCommand::TestCoreRestart),
            "heap_test" => Some(UartCommand::HeapTest),
            "net" => Some(UartCommand::Net),
            "stats" => Some(UartCommand::Stats),
            "time" => Some(UartCommand::Time),
            "beep" => Some(UartCommand::Beep),
            _ => Some(UartCommand::ShellCommand(fs::ShellCommand::parse(s).unwrap_or(fs::ShellCommand::Help))),
        }
    }

    /// Kernel debugging commands, which only root may use.
    fn root_only(&self) -> bool {
        matches!(self, UartCommand::HeapRemain | UartCommand::LastAllocs | UartCommand::OverflowHeap(_)
            | UartCommand::TestThreadTask | UartCommand::TestAsyncTask | UartCommand::DumpRegs | UartCommand::AllDump
            | UartCommand::TestCoreRestart | UartCommand::HeapTest | UartCommand::Log(_) | UartCommand::Dmesg(_)
            | UartCommand::Mmu(_) | UartCommand::Load(_))
    }

    fn execute(&self) {
        match self {
            UartCommand::HeapRemain => {
                let remain = ALLOCATOR.heap_remaining();
                println!("Heap remain: {}% ({} / {})", remain.0, remain.1, remain.2);
            }
            UartCommand::LastAllocs => {
                for alloc in ALLOCATOR.last_allocations() {
                    println!("Alloc: start={} size={}", alloc.start, alloc.size);
                }
            }
            UartCommand::OverflowHeap(s) => {
                let mut str = String::new();
                loop {
                    for b in s.bytes() {
                        str.push(b as char);
                    }
                }
            }
            UartCommand::ShellCommand(s) => {
                s.execute();
            }
            UartCommand::TestThreadTask => {
                crate::std::thread::spawn(|| { crate::id_println!("Hello from another core!;"); });
            }
            UartCommand::TestAsyncTask => {
                crate::scheduler::asyncs::test::run_async_test();
            }
            UartCommand::TestHash(s) => {
                let hash = crate::std::hash::fnv::fnv1a_64(s.as_bytes());
                println!("Hash of {} is {}", s, hash);
            }
            UartCommand::DumpRegs => {
                let regdump = RegisterDump::capture();
                println!("{}", regdump);
            }
            UartCommand::AllDump => {
                let regdump = RegisterDump::capture();
                println!("{}", regdump);
                crate::std::thread::spawn(|| {
                    let regdump = RegisterDump::capture();
                    println!("{}", regdump);
                    crate::arch::timer::delay_ms(250);
                });
                crate::std::thread::spawn(|| {
                    let regdump = RegisterDump::capture();
                    println!("{}", regdump);
                    crate::arch::timer::delay_ms(250);
                });
                crate::scheduler::asyncs::spawn(async {
                    let regdump = RegisterDump::capture();
                    println!("{}", regdump);
                    crate::arch::timer::delay_ms_async(250).await;
                });
                crate::scheduler::asyncs::spawn(async {
                    let regdump = RegisterDump::capture();
                    println!("{}", regdump);
                    crate::arch::timer::delay_ms_async(250).await;
                });
            }
            UartCommand::KShellCommand(s) => {
                s.execute();
            }
            UartCommand::Mmu(arg) => {
                let (free, total) = crate::mm::frame::stats();
                println!("MMU {}, frames: {} free / {} total", if crate::arch::mmu::is_enabled() { "on" } else { "off" }, free, total);
                if arg.is_empty() { return; }
                let Ok(va) = usize::from_str_radix(arg.trim_start_matches("0x"), 16) else {
                    println!("mmu: usage: mmu [hex address]");
                    return;
                };
                let sw = crate::mm::vm::translate(va);
                println!("{:#x}: page tables -> {:x?}, hardware -> {:x?}", va, sw, crate::arch::mmu::hw_translate(va));
            }
            UartCommand::HeapTest => crate::test::heap_stress_test(),
            UartCommand::Net => {
                let st = crate::net::status();
                if !st.device {
                    println!("net: no network device");
                    return;
                }
                println!("mac      {:02x?}", st.mac);
                match st.address {
                    Some(a) => println!("address  {}", a),
                    None => println!("address  (waiting for DHCP)"),
                }
                println!("gateway  {:?}", st.gateway);
                println!("dns      {:?}", st.dns);
            }
            UartCommand::Beep => {
                if let Err(e) = crate::sound::beep().map(drop) {
                    println!("beep: {}", e);
                }
            }
            UartCommand::Snd(arg) => {
                let hz = match *arg {
                    "off" | "stop" => Some(0),
                    a => a.parse::<u32>().ok(),
                };
                match hz {
                    Some(hz) => if let Err(e) = crate::sound::snd(hz) {
                        println!("snd: {}", e);
                    },
                    None => println!("snd: usage: snd <hz> (snd 0 or snd off for silence)"),
                }
            }
            UartCommand::Play(notes) => {
                if notes.is_empty() {
                    println!("play: usage: play <notes> or play <file.wav>, e.g. play T140 O4 C D E F G2 (see the README's Sound section)");
                    return;
                }
                let played = if crate::sound::is_wav_path(notes) {
                    crate::sound::play_file(&fs::current_cwd(), notes)
                } else {
                    crate::sound::play(notes)
                };
                if let Err(e) = played.map(drop) {
                    println!("play: {}", e);
                }
            }
            UartCommand::Ping(host) => match crate::net::ping(host).wait() {
                Ok(ms) => println!("reply from {}: time={} ms", host, ms),
                Err(e) => println!("ping {}: {}", host, e),
            },
            UartCommand::Dns(name) => match crate::std::net::lookup_host(name) {
                Ok(ip) => println!("{} is {}", name, ip),
                Err(e) => println!("dns {}: {}", name, e),
            },
            UartCommand::Browse(url) => {
                let url = if url.is_empty() { None } else { Some(*url) };
                crate::ui::window::open_app(alloc::boxed::Box::new(crate::ui::apps::browser::Browser::with_url(url)));
            }
            UartCommand::Fetch(url) => match crate::net::fetch(url).wait() {
                Ok(resp) => {
                    println!("HTTP {} {} ({} bytes)", resp.status, resp.reason, resp.body.len());
                    println!("{}", alloc::string::String::from_utf8_lossy(&resp.body));
                }
                Err(e) => println!("fetch: {}", e),
            },
            UartCommand::Kai(args) => crate::assistant::shell(args),
            UartCommand::Stats => crate::stats::print(),
            UartCommand::Log(args) => {
                let args: alloc::vec::Vec<&str> = args.split_whitespace().collect();
                let result = match args.as_slice() {
                    [] => {
                        crate::klog::print_filters();
                        println!("usage: log <level> | log <target> <level> | log <spec> | log reset");
                        println!("levels: off error warn info debug trace; targets: console, default, or a module like net, scheduler, mm::frame");
                        Ok(())
                    }
                    ["reset"] => { crate::klog::reset(); Ok(()) }
                    [spec] => crate::klog::apply_spec(spec),
                    [target, level] => match crate::klog::parse_level(level) {
                        Some(level) => crate::klog::set_level(target, level),
                        None => Err("unknown level (use off/error/warn/info/debug/trace)"),
                    },
                    _ => Err("usage: log <target> <level>"),
                };
                if let Err(e) = result {
                    println!("log: {}", e);
                }
            }
            UartCommand::Dmesg(args) => {
                // dmesg [count] [text]
                let mut count = 50;
                let mut filter = None;
                for arg in args.split_whitespace() {
                    match arg.parse::<usize>() {
                        Ok(n) => count = n,
                        Err(_) => filter = Some(arg),
                    }
                }
                crate::klog::dump(count, filter);
            }
            UartCommand::TestCoreRestart => {
                thread::spawn(crate::test::test_secondary_core_restart);
            }
            UartCommand::Load(time) => {
                let mut count = 0;
                let mut candidate: u64 = 3;
                let start_time = Instant::now();
                let time = time.parse::<u64>().unwrap_or(150);
                loop {
                    if util::is_prime(candidate) {
                        count += 1;
                        if count % 100_000 == 0 {
                            let elapsed = start_time.elapsed().as_secs_f64();
                            println!("Found {} primes. Current prime: {}. Elapsed time: {:.2}s", count, candidate, elapsed);
                        }
                    }
                    candidate += 2;
                    if start_time.elapsed().as_secs() >= time {
                        let final_elapsed = start_time.elapsed().as_secs_f64();
                        println!("{} seconds reached! Total primes found: {}. Final time: {:.2}s", time, count, final_elapsed);
                        break;
                    }
                }
            }
            UartCommand::Time => {
                use crate::drivers::rtc;
                println!("{}", rtc::local_now().unwrap_or(rtc::DateTime::new()))
            }
        }
    }
}

// ---- logging in ----------------------------------------------------------------

/// Who's logged in, then whoever `su` made the session act as on top of
/// that: the last one is who it acts as now.
type Logins = Vec<(Uid, String)>;

/// Asks for a new password twice, until it's the same both times.
fn new_password(uart: &Uart, whose: &str) -> String {
    loop {
        crate::print!("new password for {}: ", whose);
        let first = uart.read_secret();
        if first.is_empty() {
            println!("a password can't be empty");
            continue;
        }
        crate::print!("again: ");
        if uart.read_secret() == first {
            return first;
        }
        println!("those weren't the same");
    }
}

/// The first boot: makes the root account and a first user, who's then
/// logged in. Without a disk to keep them on, it's root, with no password.
fn set_up(uart: &Uart) -> (Uid, String) {
    println!("Welcome to KOS. First, its accounts: root, who can do anything, and yours.");
    let password = new_password(uart, "root");
    if let Err(e) = users::add("root", &password) {
        println!("can't set up accounts ({}); you're root until there's a disk", e);
        return (ROOT, String::from("root"));
    }
    loop {
        crate::print!("your user name: ");
        let name = uart.read_s();
        let name = name.trim();
        if !users::valid_name(name) || name == "root" {
            println!("names are 1-16 lower-case letters, digits, _ or -, starting with a letter (and not root)");
            continue;
        }
        let password = new_password(uart, name);
        match users::add(name, &password) {
            Ok(account) => {
                println!("Done. Your files go in {}; type 'help' for commands.", account.home);
                return (account.uid, account.name);
            }
            Err(e) => println!("can't add {}: {}", name, e),
        }
    }
}

/// Asks for a name and password until they're right (or sets KOS up, the
/// first time), and returns who logged in.
fn log_in(uart: &Uart) -> (Uid, String) {
    if !users::any() {
        return set_up(uart);
    }
    loop {
        crate::print!("login: ");
        let name = uart.read_s();
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        crate::print!("password: ");
        let password = uart.read_secret();
        match users::check_password(name, &password) {
            Some(account) => return (account.uid, account.name),
            None => println!("wrong name or password"),
        }
    }
}

/// Makes the session act as the last of `logins`, in their home folder.
fn become_last(logins: &Logins) {
    let (uid, name) = logins.last().unwrap();
    users::set_session_uid(*uid);
    let home = users::lookup(name).map_or(String::from("/"), |a| a.home);
    fs::go_to(&home);
}

/// Ends the programs of everyone in `logins` and closes the desktop's
/// windows (but the Terminal), then forgets them.
fn log_out(logins: &mut Logins) {
    for p in crate::process::list() {
        if logins.iter().any(|(uid, _)| *uid == p.uid) {
            let _ = crate::process::kill(p.task);
        }
    }
    crate::ui::window::close_all_but_terminal();
    logins.clear();
    users::set_session_uid(users::NOBODY);
    fs::go_to("/");
    println!("logged out");
}

/// `useradd`, `userdel`, `passwd`, `su`, `exit` and `logout`. Returns
/// whether `line` was one of them.
fn session_command(uart: &Uart, line: &str, logins: &mut Logins) -> bool {
    let (cmd, arg) = match line.split_once(' ') {
        Some((c, a)) => (c, a.trim()),
        None => (line, ""),
    };
    let (me, my_name) = logins.last().cloned().unwrap();
    match cmd {
        "logout" => log_out(logins),
        "exit" if logins.len() > 1 => {
            logins.pop();
            become_last(logins);
        }
        "exit" => log_out(logins),
        "su" => {
            let name = if arg.is_empty() { "root" } else { arg };
            let account = if me == ROOT {
                users::lookup(name)
            } else {
                crate::print!("password for {}: ", name);
                users::check_password(name, &uart.read_secret())
            };
            match account {
                Some(a) => {
                    logins.push((a.uid, a.name));
                    become_last(logins);
                }
                None if me == ROOT => println!("su: no account named {}", name),
                None => println!("su: wrong name or password"),
            }
        }
        "passwd" => {
            let name = if arg.is_empty() { my_name.as_str() } else { arg };
            if me != ROOT && name != my_name {
                println!("passwd: only root can change someone else's password");
                return true;
            }
            if users::lookup(name).is_none() {
                println!("passwd: no account named {}", name);
                return true;
            }
            if me != ROOT {
                crate::print!("current password: ");
                if users::check_password(name, &uart.read_secret()).is_none() {
                    println!("passwd: that's not it");
                    return true;
                }
            }
            let password = new_password(uart, name);
            match users::set_password(name, &password) {
                Ok(()) => println!("changed {}'s password", name),
                Err(e) => println!("passwd: {}", e),
            }
        }
        "useradd" | "userdel" if me != ROOT => println!("{}: only root can do that (try su)", cmd),
        "useradd" | "userdel" if arg.is_empty() => println!("{}: usage: {} <name>", cmd, cmd),
        "useradd" => {
            if users::lookup(arg).is_some() {
                println!("useradd: {} already exists", arg);
                return true;
            }
            if !users::valid_name(arg) {
                println!("useradd: names are 1-16 lower-case letters, digits, _ or -, starting with a letter");
                return true;
            }
            let password = new_password(uart, arg);
            match users::add(arg, &password) {
                Ok(a) => println!("added {} (uid {}, home {})", a.name, a.uid, a.home),
                Err(e) => println!("useradd: {}", e),
            }
        }
        "userdel" => {
            if logins.iter().any(|(_, n)| n == arg) {
                println!("userdel: {} is logged in", arg);
                return true;
            }
            match users::remove(arg) {
                Ok(a) => println!("removed {}; {} is still there", a.name, a.home),
                Err(e) => println!("userdel: {}", e),
            }
        }
        _ => return false,
    }
    true
}

pub extern "C" fn uart_read_task() -> ! {
    let uart = Uart {};
    crate::scheduler::follow_session();
    crate::filesys::fs_task::wait_until_ready();
    println!("KOS shell. Type 'help' for commands.");
    let mut logins = Logins::new();
    loop {
        if logins.is_empty() {
            logins.push(log_in(&uart));
            become_last(&logins);
        }
        if crate::assistant::awaiting_answer() {
            crate::print!("yes/no> ");
        } else {
            let (uid, name) = logins.last().unwrap();
            crate::print!("{}@kos:{}{} ", name, fs::current_cwd(), if *uid == ROOT { "#" } else { ">" });
        }
        let s = uart.read_s();
        if crate::assistant::awaiting_answer() {
            crate::assistant::answer(&s);
            continue;
        }
        if !s.is_empty() {
            log::debug!("command: {}", s);
            if session_command(&uart, s.trim(), &mut logins) || fs::run_pipeline(&s) {
                continue;
            }
            if let Some(command) = UartCommand::parse(&s) {
                if command.root_only() && logins.last().unwrap().0 != ROOT {
                    println!("only root can do that (try su)");
                } else {
                    command.execute();
                }
            }
        }
    }
}
