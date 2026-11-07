//! The console shell. `uart_read_task` reads console input (the serial
//! port and the Terminal window) a line at a time and runs it as a
//! command. While a program started with `run` is going, the shell waits
//! for it, and the program reads the console itself (`readln`). Programs
//! can be joined into pipelines and sent to files (see `fs`).

pub mod fs;

use alloc::borrow::ToOwned;
use alloc::string::String;
use crate::arch::stacktrace::RegisterDump;
use crate::arch::timer::Instant;
use crate::drivers::uart::Uart;
use crate::mm::allocator::ALLOCATOR;
use crate::println;
use crate::std::thread;
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

pub extern "C" fn uart_read_task() -> ! {
    let uart = Uart {};
    crate::filesys::fs_task::wait_until_ready();
    println!("KOS shell. Type 'help' for commands.");
    loop {
        if crate::assistant::awaiting_answer() {
            crate::print!("yes/no> ");
        } else {
            crate::print!("kos:{}> ", fs::current_cwd());
        }
        let s = uart.read_s();
        if crate::assistant::awaiting_answer() {
            crate::assistant::answer(&s);
            continue;
        }
        if !s.is_empty() {
            log::debug!("command: {}", s);
            if fs::run_pipeline(&s) {
                continue;
            }
            if let Some(command) = UartCommand::parse(&s) {
                command.execute();
            }
        }
    }
}
