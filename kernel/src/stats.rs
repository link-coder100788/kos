//! System-wide counters, shown by the shell's `stats` command.

use core::sync::atomic::{AtomicU64, Ordering};
use crate::{k::ast::Type::I64, println};

pub static CONTEXT_SWITCHES: AtomicU64 = AtomicU64::new(0);
pub static TASKS_SPAWNED: AtomicU64 = AtomicU64::new(0);
pub static TASKS_EXITED: AtomicU64 = AtomicU64::new(0);
pub static PROGRAMS_KILLED: AtomicU64 = AtomicU64::new(0);
pub static SYSCALLS: [AtomicU64; 19] = [const { AtomicU64::new(0) }; 19];
pub static IRQS: [AtomicU64; 128] = [const { AtomicU64::new(0) }; 128];
pub static DISK_READS: AtomicU64 = AtomicU64::new(0);
pub static DISK_WRITES: AtomicU64 = AtomicU64::new(0);
pub static FS_REQUESTS: AtomicU64 = AtomicU64::new(0);
pub static NET_RX_PACKETS: AtomicU64 = AtomicU64::new(0);
pub static NET_RX_BYTES: AtomicU64 = AtomicU64::new(0);
pub static NET_TX_PACKETS: AtomicU64 = AtomicU64::new(0);
pub static NET_TX_BYTES: AtomicU64 = AtomicU64::new(0);
pub static HTTP_REQUESTS: AtomicU64 = AtomicU64::new(0);
pub static INPUT_EVENTS: AtomicU64 = AtomicU64::new(0);
pub static FRAMES_RENDERED: AtomicU64 = AtomicU64::new(0);

pub fn inc(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
}

pub fn add(counter: &AtomicU64, n: u64) {
    counter.fetch_add(n, Ordering::Relaxed);
}

fn get(counter: &AtomicU64) -> u64 {
    counter.load(Ordering::Relaxed)
}

fn irq_name(irq: usize) -> &'static str {
    match irq as u32 {
        crate::arch::gic::TIMER_IRQ => "timer",
        crate::arch::gic::UART_IRQ => "uart",
        i if i >= crate::arch::gic::VIRTIO_IRQ_BASE => "virtio",
        _ => "?",
    }
}

pub fn print() {
    let up = crate::arch::timer::uptime_ms();
    println!("uptime            {}.{:03} s", up / 1000, up % 1000);
    for core in crate::scheduler::scheduler_cores() {
        println!("core {} idle       {}%", core, crate::scheduler::idle_percent_of(core));
    }
    println!("context switches  {}", get(&CONTEXT_SWITCHES));
    println!("tasks             {} spawned, {} exited, {} programs killed", get(&TASKS_SPAWNED), get(&TASKS_EXITED), get(&PROGRAMS_KILLED));
    let (heap_size, heap_used) = crate::mm::allocator::ALLOCATOR.heap_size();
    let (frames_free, frames_total) = crate::mm::frame::stats();
    println!("heap              {} / {} KiB used ({} KiB more to grow into)", heap_used / 1024, heap_size / 1024,
        crate::mm::allocator::ALLOCATOR.heap_remaining().1 as usize / 1024 - (heap_size - heap_used) / 1024);
    println!("page frames       {} / {} used", frames_total - frames_free, frames_total);
    println!("disk              {} reads, {} writes ({} fs requests)", get(&DISK_READS), get(&DISK_WRITES), get(&FS_REQUESTS));
    use crate::drivers::rtc;
    let check = rtc::time_check();
    println!("time              {} s unix, {}, {} s off in {} ms", rtc::unix_time().map(|v| v as i64).unwrap_or(i64::MIN), rtc::DateTime::new(), &check.as_ref().map(|v| v.1 as i64).unwrap_or(i64::MIN), &check.as_ref().map(|v| v.2 as i64).unwrap_or(i64::MIN));
    println!("network           rx {} packets / {} bytes, tx {} packets / {} bytes, {} http requests",
        get(&NET_RX_PACKETS), get(&NET_RX_BYTES), get(&NET_TX_PACKETS), get(&NET_TX_BYTES), get(&HTTP_REQUESTS));
    println!("ui                {} frames rendered, {} input events", get(&FRAMES_RENDERED), get(&INPUT_EVENTS));
    let mut line = alloc::string::String::from("interrupts       ");
    for (irq, count) in IRQS.iter().enumerate() {
        let n = get(count);
        if n > 0 {
            line.push_str(&alloc::format!(" {}({})={}", irq, irq_name(irq), n));
        }
    }
    println!("{}", line);
    let mut line = alloc::string::String::from("syscalls         ");
    for (num, count) in SYSCALLS.iter().enumerate() {
        let n = get(count);
        if n > 0 {
            line.push_str(&alloc::format!(" {}={}", crate::syscall::syscall_name(num as u64), n));
        }
    }
    println!("{}", line);
}
