//! The real-time clock: an ARM PL031 at 0x0901_0000 on QEMU's `virt`.
//!
//! The PL031 is simple: its data register (RTCDR) reads as the number of
//! seconds since 1970-01-01 00:00 UTC, ticking on its own (QEMU keeps it in
//! step with the host's clock). Everything else here is calendar maths.
//!
//! The clock holds UTC (the Makefile passes `-rtc base=utc`). Local time
//! adds the host's UTC offset, which the Makefile passes at boot through
//! fw_cfg as `opt/kos/utc_offset` (`date +%z`, e.g. "+0100").

use core::net::Ipv4Addr;
use core::ptr::read_volatile;
use core::sync::atomic::{AtomicI32, AtomicU8, Ordering};
use alloc::format;

use crate::arch::timer::Instant;
use crate::mm::layout::phys_to_virt;
use crate::std;

const BASE: usize = phys_to_virt(0x0901_0000);
/// Data register: seconds since the Unix epoch.
const RTCDR: usize = 0x000;
/// The PrimeCell ID every AMBA device has, and the PL031's own part number.
const PCELL_ID: [u8; 4] = [0x0d, 0xf0, 0x05, 0xb1];
const PERIPH_ID0: u8 = 0x31;

const NTP_SERVER: (&'static str, u16) = ("time.google.com", 123);

fn reg(offset: usize) -> u32 {
    unsafe { read_volatile((BASE + offset) as *const u32) }
}

/// 0 not checked yet, 1 present, 2 absent.
static PRESENT: AtomicU8 = AtomicU8::new(0);
/// Seconds east of UTC; i32::MIN until read from fw_cfg.
static UTC_OFFSET: AtomicI32 = AtomicI32::new(i32::MIN);

/// Whether there's a PL031 here, checked by its ID registers once.
pub fn is_present() -> bool {
    match PRESENT.load(Ordering::Relaxed) {
        1 => true,
        2 => false,
        _ => {
            let id = [0xff0, 0xff4, 0xff8, 0xffc].map(|o| reg(o) as u8);
            let found = id == PCELL_ID && reg(0xfe0) as u8 == PERIPH_ID0;
            PRESENT.store(if found { 1 } else { 2 }, Ordering::Relaxed);
            if found {
                let now = DateTime::from_unix(reg(RTCDR) as i64);
                log::info!("rtc: PL031, {} UTC, local offset {}", now, Offset(utc_offset()));
            } else {
                log::info!("rtc: no PL031 found; no real time");
            }
            found
        }
    }
}

/// Seconds since 1970-01-01 00:00 UTC, if there's a clock.
pub fn unix_time() -> Option<u64> {
    is_present().then(|| reg(RTCDR) as u64)
}

/// The local time zone's offset from UTC in seconds (0 if the host didn't
/// say).
pub fn utc_offset() -> i32 {
    let cached = UTC_OFFSET.load(Ordering::Relaxed);
    if cached != i32::MIN {
        return cached;
    }
    let mut buf = [0u8; 16];
    let offset = crate::drivers::fw_cfg::read_file("opt/kos/utc_offset", &mut buf)
        .and_then(|n| parse_offset(core::str::from_utf8(&buf[..n]).ok()?.trim_end_matches('\0').trim()))
        .unwrap_or(0);
    UTC_OFFSET.store(offset, Ordering::Relaxed);
    offset
}

/// "+0100" / "-0530" (as `date +%z` prints) -> seconds east of UTC.
fn parse_offset(s: &str) -> Option<i32> {
    let (sign, digits) = match s.as_bytes().first()? {
        b'+' => (1, &s[1..]),
        b'-' => (-1, &s[1..]),
        _ => return None,
    };
    if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let hours: i32 = digits[..2].parse().ok()?;
    let minutes: i32 = digits[2..].parse().ok()?;
    (hours <= 14 && minutes < 60).then_some(sign * (hours * 3600 + minutes * 60))
}

/// The current local date and time, if there's a clock.
pub fn local_now() -> Option<DateTime> {
    unix_time().map(|t| DateTime::from_unix(t as i64 + utc_offset() as i64))
}

/// A calendar date and time of day.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateTime {
    pub year: i32,
    /// 1 to 12.
    pub month: u8,
    /// 1 to 31.
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    /// 0 is Sunday.
    pub weekday: u8,
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

impl DateTime {
    /// from_unix shorthand
    pub fn new() -> Self {
        DateTime::from_unix(unix_time().unwrap_or(0) as i64)
    }

    /// The date and time `secs` seconds after 1970-01-01 00:00.
    pub fn from_unix(secs: i64) -> DateTime {
        let days = secs.div_euclid(86_400);
        let of_day = secs.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        DateTime {
            year,
            month,
            day,
            hour: (of_day / 3600) as u8,
            minute: (of_day / 60 % 60) as u8,
            second: (of_day % 60) as u8,
            // 1970-01-01 was a Thursday.
            weekday: (days + 4).rem_euclid(7) as u8,
        }
    }

    /// "Jan" .. "Dec".
    pub fn month_name(&self) -> &'static str {
        MONTHS[(self.month - 1) as usize]
    }

    /// "Sun" .. "Sat".
    pub fn weekday_name(&self) -> &'static str {
        WEEKDAYS[self.weekday as usize]
    }
}

/// Year, month and day for a count of days since 1970-01-01, using Howard
/// Hinnant's `civil_from_days`: shift the year to start in March, so the
/// leap day is the last day of the year, then work in 400-year eras (each
/// exactly 146097 days long).
fn civil_from_days(days: i64) -> (i32, u8, u8) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // day of era, 0..146096
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // year of era, 0..399
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day of (March-based) year, 0..365
    let mp = (5 * doy + 2) / 153; // March-based month, 0..11
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let year = (yoe + era * 400 + i64::from(month <= 2)) as i32;
    (year, month, day)
}

impl core::fmt::Display for DateTime {
    /// "2026-10-27 18:42:07".
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(f, "{:04}-{:02}-{:02} {:02}:{:02}:{:02}", self.year, self.month, self.day, self.hour, self.minute, self.second)
    }
}

/// Shows a UTC offset as "+01:00".
struct Offset(i32);

impl core::fmt::Display for Offset {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        let sign = if self.0 < 0 { '-' } else { '+' };
        let a = self.0.unsigned_abs();
        write!(f, "{}{:02}:{:02}", sign, a / 3600, a / 60 % 60)
    }
}

/// Checks if our time is within 1s of ntp, and returns the diff
pub fn time_check() -> std::io::Result<(bool, u64, u64)> {
    use crate::std::net::*;
    use crate::net::ntp::*;
    let now = Instant::now();
    let sock = UdpSocket::bind("0.0.0.0:0")?;
    let server: (Ipv4Addr, u16) = (lookup_host(NTP_SERVER.0).unwrap_or(Ipv4Addr::new(0, 0, 0, 0)), NTP_SERVER.1);
    match NtpClient::query_time(&sock, server) {
        Ok(result) => {
            let sys_unix = unix_time().unwrap_or(0);
            return Ok((sys_unix.abs_diff(result.unix_seconds) <= 3, sys_unix.abs_diff(result.unix_seconds), now.elapsed().as_millis() as u64));
        }
        Err(e) => { return Err(std::io::Error::other(format!("{}", e))) }
    }
}
