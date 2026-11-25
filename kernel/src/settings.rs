//! Global settings: one set for the whole machine (not per user), kept in
//! `/.kos/settings` as `key = value` lines. Change them with the Settings
//! window (in the KOS menu) or `settings` at the console; either saves the
//! file and takes effect straight away. Anyone logged in may change them.
//!
//! Each setting is an entry in `ALL`. What uses it reads it with `get` (or
//! a typed shortcut below, kept in an atomic for code that runs often,
//! like the sound mixer). The desktop notices changes through
//! `generation` and applies the ones it owns, like the screen resolution,
//! on the UI task.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use crate::filesys::fs_ipc::{fs_call_as, FsRequest, FsResponse};
use crate::std::lock::irq_mutex::IrqMutex;
use crate::users::ROOT;

pub const SETTINGS_FILE: &str = "/.kos/settings";

/// What values a setting takes.
pub enum Kind {
    /// One of these.
    Choice(&'static [&'static str]),
    /// on or off.
    Switch,
    /// A whole number from `min` to `max` in steps of `step`.
    Number { min: i32, max: i32, step: i32, unit: &'static str },
    /// "auto" (the host's) or a UTC offset like +01:00.
    Offset,
    /// Free text, checked by `check` (which returns what's wrong).
    Text { max: usize, check: fn(&str) -> Option<&'static str> },
}

pub struct Setting {
    pub key: &'static str,
    /// What the Settings window calls it.
    pub label: &'static str,
    pub help: &'static str,
    pub kind: Kind,
    pub default: &'static str,
}

/// Screen sizes KOS offers. The display can do others, but these are the
/// common ones QEMU's window shows well.
pub const RESOLUTIONS: &[&str] = &[
    "800x600", "1024x768", "1280x720", "1280x800", "1280x1024", "1366x768", "1440x900", "1600x900",
    "1680x1050", "1920x1080", "1920x1200",
];

/// Desktop backgrounds: name, top and bottom of the gradient, and the
/// color of the KOS logo on it.
pub const WALLPAPERS: &[(&str, u32, u32, u32)] = &[
    ("dusk", 0x001b2a49, 0x00462c5a, 0x005a5e8a),
    ("ocean", 0x000b3d5c, 0x00137a7f, 0x002f8c9c),
    ("forest", 0x00183a26, 0x004a5a2a, 0x00527a4a),
    ("sunset", 0x006a2c4a, 0x00d0763a, 0x00d88a6a),
    ("graphite", 0x00303338, 0x0016181c, 0x00505660),
];

pub const ALL: &[Setting] = &[
    Setting {
        key: "display.resolution", label: "Screen resolution", default: "1024x768",
        help: "Width x height of the screen, in pixels",
        kind: Kind::Choice(RESOLUTIONS),
    },
    Setting {
        key: "desktop.wallpaper", label: "Wallpaper", default: "dusk",
        help: "The desktop's background",
        kind: Kind::Choice(&["dusk", "ocean", "forest", "sunset", "graphite"]),
    },
    Setting {
        key: "desktop.logo", label: "KOS logo on desktop", default: "on",
        help: "The big KOS in the desktop's corner",
        kind: Kind::Switch,
    },
    Setting {
        key: "clock.format", label: "Clock", default: "24h",
        help: "The taskbar clock: 24-hour or 12-hour (am/pm)",
        kind: Kind::Choice(&["24h", "12h"]),
    },
    Setting {
        key: "clock.seconds", label: "Clock shows seconds", default: "off",
        help: "Seconds on the taskbar clock",
        kind: Kind::Switch,
    },
    Setting {
        key: "clock.date", label: "Clock shows date", default: "on",
        help: "The day and date before the taskbar clock's time",
        kind: Kind::Switch,
    },
    Setting {
        key: "clock.utc_offset", label: "Time zone", default: "auto",
        help: "Local time's offset from UTC, like +01:00 or -05:30; auto takes the host's",
        kind: Kind::Offset,
    },
    Setting {
        key: "mouse.double_click", label: "Double-click speed", default: "400",
        help: "How quickly (ms) a second click on a title bar must follow to maximize",
        kind: Kind::Number { min: 200, max: 1000, step: 100, unit: "ms" },
    },
    Setting {
        key: "sound.volume", label: "Volume", default: "100",
        help: "Loudness of everything KOS plays, as a percentage",
        kind: Kind::Number { min: 0, max: 100, step: 10, unit: "%" },
    },
    Setting {
        key: "system.hostname", label: "Computer name", default: "kos",
        help: "This computer's name: in the console prompt and uname",
        kind: Kind::Text { max: 32, check: check_hostname },
    },
    Setting {
        key: "browser.home", label: "Browser start page", default: "",
        help: "A URL the Browser opens with; empty for its own start page",
        kind: Kind::Text { max: 200, check: check_url },
    },
];

fn check_hostname(s: &str) -> Option<&'static str> {
    let ok = !s.is_empty()
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !s.starts_with('-');
    (!ok).then_some("names are lower-case letters, digits and -, not starting with -")
}

fn check_url(s: &str) -> Option<&'static str> {
    s.contains(char::is_whitespace).then_some("a URL can't have spaces")
}

/// Current values, in `ALL`'s order; empty until `load`.
static VALUES: IrqMutex<Vec<String>> = IrqMutex::new(Vec::new());
/// Bumped by every change, so the desktop can tell when to look again.
static GENERATION: AtomicU32 = AtomicU32::new(0);

// Copies of settings read often, kept up to date by `applied`.
static VOLUME: AtomicU32 = AtomicU32::new(100);
static DOUBLE_CLICK_MS: AtomicU32 = AtomicU32::new(400);
/// Seconds east of UTC, or i32::MIN for the host's.
static UTC_OFFSET: AtomicI32 = AtomicI32::new(i32::MIN);

pub fn find(key: &str) -> Option<usize> {
    ALL.iter().position(|s| s.key == key)
}

/// A setting's value (its default if it isn't set or doesn't exist).
pub fn get(key: &str) -> String {
    let Some(i) = find(key) else { return String::new() };
    VALUES.lock().get(i).cloned().unwrap_or_else(|| ALL[i].default.to_string())
}

pub fn generation() -> u32 {
    GENERATION.load(Ordering::Acquire)
}

pub fn resolution() -> (usize, usize) {
    parse_resolution(&get("display.resolution")).unwrap_or((1024, 768))
}

pub fn is_on(key: &str) -> bool {
    get(key) == "on"
}

pub fn volume() -> u32 {
    VOLUME.load(Ordering::Relaxed)
}

pub fn double_click_ms() -> u64 {
    DOUBLE_CLICK_MS.load(Ordering::Relaxed) as u64
}

/// The time zone's offset in seconds, unless it's left to the host.
pub fn utc_offset() -> Option<i32> {
    let offset = UTC_OFFSET.load(Ordering::Relaxed);
    (offset != i32::MIN).then_some(offset)
}

/// The wallpaper's gradient top, bottom, and logo color.
pub fn wallpaper() -> (u32, u32, u32) {
    let name = get("desktop.wallpaper");
    let (_, top, bottom, logo) = WALLPAPERS.iter().find(|w| w.0 == name).unwrap_or(&WALLPAPERS[0]);
    (*top, *bottom, *logo)
}

pub fn parse_resolution(s: &str) -> Option<(usize, usize)> {
    let (w, h) = s.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

/// "+01:00", "+0100", "-5:30", "+1" -> seconds east of UTC.
fn parse_offset(s: &str) -> Option<i32> {
    let (sign, rest) = match s.as_bytes().first()? {
        b'+' => (1, &s[1..]),
        b'-' => (-1, &s[1..]),
        _ => return None,
    };
    let (hours, minutes) = match rest.split_once(':') {
        Some((h, m)) => (h, m),
        None if rest.len() == 4 => (&rest[..2], &rest[2..]),
        None => (rest, "0"),
    };
    let all_digits = |t: &str| !t.is_empty() && t.len() <= 2 && t.bytes().all(|b| b.is_ascii_digit());
    if !all_digits(hours) || !all_digits(minutes) {
        return None;
    }
    let (hours, minutes): (i32, i32) = (hours.parse().ok()?, minutes.parse().ok()?);
    (hours <= 14 && minutes < 60).then_some(sign * (hours * 3600 + minutes * 60))
}

/// Seconds east of UTC as "+01:00".
fn format_offset(secs: i32) -> String {
    let sign = if secs < 0 { '-' } else { '+' };
    format!("{}{:02}:{:02}", sign, secs.abs() / 3600, secs.abs() / 60 % 60)
}

/// `value` as setting `s` stores it, or what's wrong with it.
pub fn normalize(s: &Setting, value: &str) -> Result<String, String> {
    let value = value.trim();
    let wrong = |what: String| Err(format!("{}: {}", s.key, what));
    match &s.kind {
        Kind::Choice(choices) => match choices.iter().find(|c| c.eq_ignore_ascii_case(value)) {
            Some(c) => Ok(c.to_string()),
            None => wrong(format!("pick one of {}", choices.join(", "))),
        },
        Kind::Switch => match value.to_ascii_lowercase().as_str() {
            "on" | "yes" | "true" | "1" => Ok(String::from("on")),
            "off" | "no" | "false" | "0" => Ok(String::from("off")),
            _ => wrong(String::from("on or off")),
        },
        Kind::Number { min, max, unit, .. } => match value.trim_end_matches(unit).trim().parse::<i32>() {
            Ok(n) if (*min..=*max).contains(&n) => Ok(n.to_string()),
            _ => wrong(format!("a number from {} to {}", min, max)),
        },
        Kind::Offset if value.eq_ignore_ascii_case("auto") => Ok(String::from("auto")),
        Kind::Offset => match parse_offset(value) {
            Some(secs) => Ok(format_offset(secs)),
            None => wrong(String::from("auto, or an offset like +01:00 or -05:30")),
        },
        Kind::Text { max, check } => {
            if value.len() > *max {
                return wrong(format!("at most {} characters", max));
            }
            match check(value) {
                Some(e) => wrong(String::from(e)),
                None => Ok(value.to_string()),
            }
        }
    }
}

/// The value after (`forward`) or before `value`, for the Settings window's
/// arrows. Text has none.
pub fn step(s: &Setting, value: &str, forward: bool) -> Option<String> {
    let cycle = |n: usize, at: usize| if forward { (at + 1) % n } else { (at + n - 1) % n };
    match &s.kind {
        Kind::Choice(choices) => {
            let at = choices.iter().position(|c| *c == value).unwrap_or(0);
            Some(choices[cycle(choices.len(), at)].to_string())
        }
        Kind::Switch => Some(String::from(if value == "on" { "off" } else { "on" })),
        Kind::Number { min, max, step, .. } => {
            let n: i32 = value.parse().unwrap_or(*min);
            let n = if forward { n + step } else { n - step };
            Some(n.clamp(*min, *max).to_string())
        }
        Kind::Offset => {
            // auto, then whole hours from -12:00 to +14:00.
            let hours: Vec<Option<i32>> = core::iter::once(None).chain((-12..=14).map(Some)).collect();
            let now = parse_offset(value).map(|s| s / 3600);
            let at = hours.iter().position(|h| *h == now).unwrap_or(0);
            Some(match hours[cycle(hours.len(), at)] {
                None => String::from("auto"),
                Some(h) => format_offset(h * 3600),
            })
        }
        Kind::Text { .. } => None,
    }
}

/// Updates the copies of settings read often.
fn applied(values: &[String]) {
    let value = |key: &str| find(key).and_then(|i| values.get(i)).map_or("", String::as_str);
    VOLUME.store(value("sound.volume").parse().unwrap_or(100), Ordering::Relaxed);
    DOUBLE_CLICK_MS.store(value("mouse.double_click").parse().unwrap_or(400), Ordering::Relaxed);
    UTC_OFFSET.store(parse_offset(value("clock.utc_offset")).unwrap_or(i32::MIN), Ordering::Relaxed);
    GENERATION.fetch_add(1, Ordering::AcqRel);
    crate::ui::task::wake_ui();
}

/// Values from the file's text: defaults for what it doesn't set (or sets
/// wrongly, which is logged).
fn parse(text: &str) -> Vec<String> {
    let mut values: Vec<String> = ALL.iter().map(|s| s.default.to_string()).collect();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let Some((key, value)) = line.split_once('=') else { continue };
        let Some(i) = find(key.trim()) else {
            log::warn!("{}: unknown setting {}", SETTINGS_FILE, key.trim());
            continue;
        };
        match normalize(&ALL[i], value) {
            Ok(v) => values[i] = v,
            Err(e) => log::warn!("{}: {}", SETTINGS_FILE, e),
        }
    }
    values
}

fn to_text(values: &[String]) -> String {
    let mut text = String::from("# KOS settings, for everyone on this computer. Change them with Settings\n\
        # in the KOS menu, or `settings` at the console (then `settings reload`\n\
        # after editing this file).\n");
    for (s, v) in ALL.iter().zip(values) {
        text += &format!("\n# {}\n{} = {}\n", s.help, s.key, v);
    }
    text
}

/// Reads `/.kos/settings` (once the file system is up).
pub fn load() {
    let values = match fs_call_as(ROOT, FsRequest::Read { cwd: "/".into(), path: SETTINGS_FILE.into() }) {
        FsResponse::Data(bytes) => parse(&String::from_utf8_lossy(&bytes)),
        _ => parse(""),
    };
    applied(&values);
    *VALUES.lock() = values;
}

fn save(values: &[String]) -> Result<(), String> {
    let _ = fs_call_as(ROOT, FsRequest::Mkdir { cwd: "/".into(), path: "/.kos".into() });
    match fs_call_as(ROOT, FsRequest::Write { cwd: "/".into(), path: SETTINGS_FILE.into(), data: to_text(values).into_bytes() }) {
        FsResponse::Ok => Ok(()),
        FsResponse::Err(e) => Err(format!("can't save {}: {}", SETTINGS_FILE, e)),
        _ => Err(format!("can't save {}", SETTINGS_FILE)),
    }
}

/// Changes a setting (or puts it back to its default, for None), saves the
/// file and applies it. Returns the value it now has.
pub fn set(key: &str, value: Option<&str>) -> Result<String, String> {
    let i = find(key).ok_or_else(|| format!("no setting called {} (try `settings`)", key))?;
    let value = match value {
        Some(v) => normalize(&ALL[i], v)?,
        None => ALL[i].default.to_string(),
    };
    let values = {
        let mut values = VALUES.lock();
        if values.len() != ALL.len() {
            *values = ALL.iter().map(|s| s.default.to_string()).collect();
        }
        values[i] = value.clone();
        values.clone()
    };
    log::debug!("setting {} = {}", key, value);
    applied(&values);
    save(&values)?;
    Ok(value)
}

/// Puts every setting back to its default.
pub fn reset_all() -> Result<(), String> {
    let values: Vec<String> = ALL.iter().map(|s| s.default.to_string()).collect();
    *VALUES.lock() = values.clone();
    applied(&values);
    save(&values)
}

/// The `settings` console command.
pub fn shell(args: &str) {
    use crate::println;
    let args: Vec<&str> = args.split_whitespace().collect();
    let result = match args.as_slice() {
        [] => {
            for s in ALL {
                println!("{:<20} {}", s.key, get(s.key));
            }
            println!("settings <name> to explain one, settings <name> <value> to change it,");
            println!("settings reset [name], settings reload (after editing {})", SETTINGS_FILE);
            Ok(())
        }
        ["reload"] => { load(); println!("read {}", SETTINGS_FILE); Ok(()) }
        ["reset"] => reset_all().map(|()| println!("every setting is back to its default")),
        ["reset", key] => set(key, None).map(|v| println!("{} = {}", key, v)),
        [key] => match find(key) {
            Some(i) => {
                let s = &ALL[i];
                println!("{} = {}  ({}; default {})", s.key, get(s.key), s.help,
                    if s.default.is_empty() { "empty" } else { s.default });
                match &s.kind {
                    Kind::Choice(choices) => println!("  one of: {}", choices.join(", ")),
                    Kind::Switch => println!("  on or off"),
                    Kind::Number { min, max, unit, .. } => println!("  {} to {} {}", min, max, unit),
                    Kind::Offset => println!("  auto, or like +01:00"),
                    Kind::Text { .. } => {}
                }
                Ok(())
            }
            None => Err(format!("no setting called {}", key)),
        },
        [key, value @ ..] => set(key, Some(&value.join(" "))).map(|v| println!("{} = {}", key, v)),
    };
    if let Err(e) = result {
        println!("settings: {}", e);
    }
}

#[cfg(feature = "test")]
pub mod tests {
    use super::*;

    fn test_settings_file_round_trip() {
        let mut values: Vec<String> = ALL.iter().map(|s| s.default.to_string()).collect();
        values[find("display.resolution").unwrap()] = String::from("1920x1080");
        values[find("browser.home").unwrap()] = String::from("https://example.com/");
        assert!(parse(&to_text(&values)) == values);
    }

    fn test_bad_settings_lines_keep_defaults() {
        let values = parse("display.resolution = 9x9\nnope = 1\nsound.volume=55\nclock.seconds = yes\n");
        assert_eq!(values[find("display.resolution").unwrap()], "1024x768");
        assert_eq!(values[find("sound.volume").unwrap()], "55");
        assert_eq!(values[find("clock.seconds").unwrap()], "on");
    }

    fn test_settings_values_are_checked() {
        let s = &ALL[find("clock.utc_offset").unwrap()];
        assert_eq!(normalize(s, "+1").unwrap(), "+01:00");
        assert_eq!(normalize(s, "-0530").unwrap(), "-05:30");
        assert_eq!(normalize(s, "AUTO").unwrap(), "auto");
        assert!(normalize(s, "+15").is_err());
        let host = &ALL[find("system.hostname").unwrap()];
        assert!(normalize(host, "my-mac").is_ok());
        assert!(normalize(host, "My Mac").is_err());
        let volume = &ALL[find("sound.volume").unwrap()];
        assert_eq!(normalize(volume, "30%").unwrap(), "30");
        assert!(normalize(volume, "130").is_err());
    }

    fn test_settings_step_wraps_and_clamps() {
        let res = &ALL[find("display.resolution").unwrap()];
        assert_eq!(step(res, "1920x1200", true).unwrap(), "800x600");
        let volume = &ALL[find("sound.volume").unwrap()];
        assert_eq!(step(volume, "100", true).unwrap(), "100");
        let tz = &ALL[find("clock.utc_offset").unwrap()];
        assert_eq!(step(tz, "auto", true).unwrap(), "-12:00");
        assert_eq!(step(tz, "auto", false).unwrap(), "+14:00");
        // Every listed resolution parses.
        assert!(RESOLUTIONS.iter().all(|r| parse_resolution(r).is_some()));
    }

    pub fn run() {
        crate::test::run_all(&[
            &test_settings_file_round_trip,
            &test_bad_settings_lines_keep_defaults,
            &test_settings_values_are_checked,
            &test_settings_step_wraps_and_clamps,
        ]);
    }
}
