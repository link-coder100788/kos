use core::{arch::asm, ops::Sub, time::Duration};
use crate::println;

/// Timer interrupts per second, i.e. scheduler time slices of 10 ms.
const TICK_HZ: u64 = 100;

fn read_cntfrq() -> u64 {
    let v: u64;
    unsafe { asm!("mrs {0}, cntfrq_el0", out(reg) v); }
    v
}

fn set_cntp_tval(v: u64) {
    unsafe { asm!("msr cntp_tval_el0, {0}", in(reg) v); }
}

fn enable_cntp() {
    unsafe { asm!("msr cntp_ctl_el0, {0}", in(reg) 1u64); }
}

pub fn init() {
    let freq = read_cntfrq();
    log::debug!("timer: {} Hz counter, {} Hz scheduler tick", freq, TICK_HZ);
    init_this_core();
    unsafe { asm!("msr daifclr, #2"); }
}

/// Starts this core's tick. Every core has its own timer; IRQs stay masked
/// until the core's scheduler starts.
pub fn init_this_core() {
    set_cntp_tval(read_cntfrq() / TICK_HZ);
    enable_cntp();
}

fn read_cntpct() -> u64 {
    let v: u64;
    unsafe { asm!("mrs {0}, cntpct_el0", out(reg) v); }
    v
}


/// The current timer counter value.
pub fn now() -> u64 {
    read_cntpct()
}

/// Counter ticks per second.
pub fn frequency() -> u64 {
    read_cntfrq()
}

/// Milliseconds since the machine started.
pub fn uptime_ms() -> u64 {
    read_cntpct() / (read_cntfrq() / 1000)
}

/// The counter value `ms` milliseconds from now.
pub fn deadline_after_ms(ms: u64) -> u64 {
    read_cntpct() + read_cntfrq() * ms / 1000
}

/// Waits `ms` milliseconds: blocks the task on core 0 once the scheduler
/// runs, busy-waits anywhere else (early boot, secondary cores).
pub fn delay_ms(ms: u64) {
    if crate::scheduler::can_yield() {
        crate::scheduler::wait::sleep_ms(ms);
        return;
    }
    let deadline = deadline_after_ms(ms);
    while read_cntpct() < deadline {
        core::hint::spin_loop();
    }
}

/// A future that completes `ms` milliseconds after it's created, without
/// blocking the executor in the meantime.
pub struct Delay {
    start: u64,
    ticks: u64,
}

impl core::future::Future for Delay {
    type Output = ();

    fn poll(self: core::pin::Pin<&mut Self>, cx: &mut core::task::Context<'_>) -> core::task::Poll<()> {
        if read_cntpct().wrapping_sub(self.start) >= self.ticks {
            core::task::Poll::Ready(())
        } else {
            // No timer wakeups yet, so ask to be polled again.
            cx.waker().wake_by_ref();
            core::task::Poll::Pending
        }
    }
}

pub fn delay_ms_async(ms: u64) -> Delay {
    Delay { start: read_cntpct(), ticks: (read_cntfrq() * ms) / 1000 }
}

pub static TICKS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

pub fn on_tick() {
    TICKS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    crate::scheduler::account_tick();
    set_cntp_tval(read_cntfrq() / TICK_HZ);
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Instant {
    ticks: u64,
}

impl Instant {
    #[inline]
    pub fn now() -> Self { Self { ticks: read_cntpct() } }

    #[inline]
    pub fn elapsed(self) -> Duration { Self::now() - self }

    #[inline]
    pub fn duration_since(self, other: Instant) -> Duration { self - other }
}

impl Sub for Instant {
    type Output = Duration;

    fn sub(self, rhs: Self) -> Self::Output {
        let ticks = self.ticks.wrapping_sub(rhs.ticks);
        let freq = read_cntfrq();
        let nanos = (ticks as u128 * 1_000_000_000u128) / freq as u128;
        Duration::from_nanos(nanos as u64)
    }
}

macro_rules! time {
    ($expr:expr) => {
        let now = Instant::now();
        let result = $expr;
        println!("time: {:?}", now.elapsed());
        result
    };
}