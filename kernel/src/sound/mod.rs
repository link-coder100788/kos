//! The sound system: one voice, so one sound at a time, like a PC speaker
//! (or TempleOS's `Snd`). A square-wave tone (`snd`), a short beep, a
//! song (`play`) or a WAV file (`play_file`); starting any of them
//! replaces whatever was playing.
//!
//! The `sound` task keeps the device's stream fed with periods it fills
//! from `VOICE`, silence included. Songs advance by samples as they're
//! filled, so their timing is exact whatever the scheduler does.
//!
//! Songs are written in a small music notation (MML-style):
//!   C D E F G A B   notes, with `#` or `+` for sharp and `-` for flat,
//!                   then an optional length (1 whole, 2, 4 quarter, 8, 16,
//!                   32; `L` sets the default, 4 to start) and `.` for dotted
//!   R or P          a rest, with a length like a note
//!   O0..O8          the octave (4 to start; A4 is 440 Hz), `<` / `>` one down / up
//!   T32..T255       the tempo in quarter notes a minute (120 to start)
//! Spaces are ignored, and so is case: `T140 O4 C D E8 F#8 G2`.
//!
//! Programs' `play` and `beep` wait for their sound to finish, like
//! TempleOS's (see `syscall_sound`); the kernel's don't. A program's
//! sound stops when it ends.

pub mod wav;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use crate::drivers::virtio::snd;
use crate::scheduler::{self, TaskId};
use crate::std::lock::irq_mutex::IrqMutex;

/// One note of a song: `hz` 0 is a rest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Note {
    pub hz: u32,
    pub ms: u32,
}

enum Sound {
    Silent,
    Tone(u32),
    Song { notes: Vec<Note>, at: usize, left: u32 },
    /// Mono samples at the device's rate (see `wav`).
    Samples { data: Vec<i16>, at: usize },
}

struct Voice {
    sound: Sound,
    /// The program that started it, which stops it when it ends.
    owner: Option<TaskId>,
    /// Which sound this is: one more for each sound started.
    id: u64,
}

static VOICE: IrqMutex<Voice> = IrqMutex::new(Voice { sound: Sound::Silent, owner: None, id: 0 });
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// A desktop sound waiting for the sound task (see `ui_beep`). Set from
/// anywhere, fault handlers included, so it's just a number.
static UI_REQUEST: AtomicU8 = AtomicU8::new(UI_NONE);
const UI_NONE: u8 = 0;
const UI_BEEP: u8 = 1;
const UI_BONK: u8 = 2;

const MIN_HZ: u32 = 20;
const MAX_HZ: u32 = 20_000;
/// Square-wave amplitude: loud enough, without being harsh.
const AMPLITUDE: i32 = 6000;
/// Samples to fade in or out over (2 ms at 48 kHz), so starts and stops don't click.
const RAMP: i32 = 96;

pub fn available() -> bool {
    snd::available()
}

/// The calling program, if a program is calling.
fn caller() -> Option<TaskId> {
    let id = scheduler::current_id();
    crate::process::is_running(id).then_some(id)
}

/// Replaces whatever is playing with `sound`, and returns its id (see `finished`).
fn set(sound: Sound) -> Result<u64, String> {
    if !available() {
        return Err(String::from("no sound device"));
    }
    let owner = match sound {
        Sound::Silent => None,
        _ => caller(),
    };
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let old = {
        let mut voice = VOICE.lock();
        voice.owner = owner;
        voice.id = id;
        core::mem::replace(&mut voice.sound, sound)
    };
    // Any song's notes are freed outside the lock.
    drop(old);
    Ok(id)
}

/// Plays a steady tone of `hz` until something else plays; 0 is silence.
pub fn snd(hz: u32) -> Result<(), String> {
    let sound = if hz == 0 { Sound::Silent } else { Sound::Tone(hz.clamp(MIN_HZ, MAX_HZ)) };
    set(sound).map(drop)
}

pub fn stop() -> Result<(), String> {
    set(Sound::Silent).map(drop)
}

/// A short beep, in the background: returns its id for `finished`.
pub fn beep() -> Result<u64, String> {
    set(song(vec![Note { hz: 880, ms: 120 }]))
}

/// Plays a song in the notation above, in the background: returns its id
/// for `finished`.
pub fn play(notation: &str) -> Result<u64, String> {
    let notes = parse(notation)?;
    set(song(notes))
}

/// Plays mono samples at the device's rate, in the background: returns
/// its id for `finished`.
pub fn play_samples(data: Vec<i16>) -> Result<u64, String> {
    set(if data.is_empty() { Sound::Silent } else { Sound::Samples { data, at: 0 } })
}

/// The device's sample rate.
pub fn rate() -> u32 {
    snd::with(|dev| dev.rate).unwrap_or(48000)
}

/// Plays a WAV file's contents, in the background.
pub fn play_wav(bytes: &[u8]) -> Result<u64, String> {
    if !available() {
        return Err(String::from("no sound device"));
    }
    play_samples(wav::decode(bytes, rate())?)
}

/// Plays the WAV file at `path` (relative to `cwd`), in the background.
pub fn play_file(cwd: &str, path: &str) -> Result<u64, String> {
    use crate::filesys::fs_ipc::{fs_call, FsRequest, FsResponse};
    if !available() {
        return Err(String::from("no sound device"));
    }
    match fs_call(FsRequest::Read { cwd: cwd.to_string(), path: path.to_string() }) {
        FsResponse::Data(bytes) => play_wav(&bytes),
        FsResponse::Err(e) => Err(format!("{}: {:?}", path, e)),
        _ => Err(format!("{}: can't read it", path)),
    }
}

/// Whether `arg` names a WAV file rather than being notes.
pub fn is_wav_path(arg: &str) -> bool {
    arg.len() > 4 && arg[arg.len() - 4..].eq_ignore_ascii_case(".wav") && !arg.contains(' ')
}

/// Whether sound `id` is over: it ended, or something replaced it.
pub fn finished(id: u64) -> bool {
    let voice = VOICE.lock();
    voice.id != id || matches!(voice.sound, Sound::Silent)
}

fn song(notes: Vec<Note>) -> Sound {
    if notes.is_empty() { Sound::Silent } else { Sound::Song { notes, at: 0, left: 0 } }
}

/// Whether anything is playing.
pub fn busy() -> bool {
    !matches!(VOICE.lock().sound, Sound::Silent)
}

/// A desktop beep (the Terminal's BEL): only if nothing else is playing,
/// so it never cuts off someone's song. Safe from any context.
pub fn ui_beep() {
    UI_REQUEST.store(UI_BEEP, Ordering::Relaxed);
}

/// A low "bonk" for a program that crashed. Safe from any context.
pub fn ui_bonk() {
    UI_REQUEST.store(UI_BONK, Ordering::Relaxed);
}

/// Called when task `id` has ended: stops any sound it started.
pub fn task_ended(id: TaskId) {
    let old = {
        let mut voice = VOICE.lock();
        if voice.owner != Some(id) {
            return;
        }
        voice.owner = None;
        core::mem::replace(&mut voice.sound, Sound::Silent)
    };
    drop(old);
}

// ---- notation --------------------------------------------------------------------

/// Semitones above C of a note letter.
fn semitone(letter: u8) -> Option<i32> {
    Some(match letter {
        b'C' => 0, b'D' => 2, b'E' => 4, b'F' => 5, b'G' => 7, b'A' => 9, b'B' => 11,
        _ => return None,
    })
}

/// The frequency of MIDI note `n` (69 is A4, 440 Hz).
fn note_hz(n: i32) -> u32 {
    libm::round(440.0 * libm::pow(2.0, (n - 69) as f64 / 12.0)) as u32
}

/// Turns notation into notes, or says where it went wrong.
pub fn parse(notation: &str) -> Result<Vec<Note>, String> {
    let s = notation.as_bytes();
    let mut i = 0;
    let (mut tempo, mut octave, mut length) = (120u32, 4i32, 4u32);
    let mut notes = Vec::new();

    // The number at `i`, if there is one.
    let number = |i: &mut usize| -> Option<u32> {
        let start = *i;
        while *i < s.len() && s[*i].is_ascii_digit() && *i - start < 4 {
            *i += 1;
        }
        core::str::from_utf8(&s[start..*i]).ok()?.parse().ok()
    };
    let bad = |at: usize, why: &str| -> String {
        format!("{} at character {} ('{}')", why, at + 1, s.get(at).map_or('?', |&c| c as char))
    };

    while i < s.len() {
        let at = i;
        let c = s[i].to_ascii_uppercase();
        i += 1;
        match c {
            b' ' | b'\t' | b'\n' | b'\r' => {}
            b'T' => match number(&mut i) {
                Some(t @ 32..=255) => tempo = t,
                _ => return Err(bad(at, "tempo must be T32 to T255")),
            },
            b'O' => match number(&mut i) {
                Some(o @ 0..=8) => octave = o as i32,
                _ => return Err(bad(at, "octave must be O0 to O8")),
            },
            b'L' => match number(&mut i) {
                Some(l @ 1..=64) => length = l,
                _ => return Err(bad(at, "length must be L1 to L64")),
            },
            b'<' if octave > 0 => octave -= 1,
            b'>' if octave < 8 => octave += 1,
            b'<' | b'>' => return Err(bad(at, "octave out of range")),
            b'A'..=b'G' | b'R' | b'P' => {
                let mut n = semitone(c).map(|st| (octave + 1) * 12 + st);
                if let Some(n) = n.as_mut() {
                    match s.get(i) {
                        Some(b'#' | b'+') => { *n += 1; i += 1; }
                        Some(b'-') => { *n -= 1; i += 1; }
                        _ => {}
                    }
                }
                let len = match s.get(i) {
                    Some(d) if d.is_ascii_digit() => match number(&mut i) {
                        Some(l @ 1..=64) => l,
                        _ => return Err(bad(at, "note length must be 1 to 64")),
                    },
                    _ => length,
                };
                // A whole note is four beats.
                let whole = 240_000 / tempo;
                let mut ms = whole / len;
                let mut extra = ms / 2;
                while s.get(i) == Some(&b'.') {
                    ms += extra;
                    extra /= 2;
                    i += 1;
                }
                notes.push(Note { hz: n.map_or(0, note_hz), ms: ms.max(1) });
            }
            _ => return Err(bad(at, "unexpected character")),
        }
    }
    Ok(notes)
}

// ---- the sound task --------------------------------------------------------------

/// What the voice wants for the next sample.
#[derive(Clone, Copy)]
enum Out {
    Quiet,
    Tone(u32),
    Sample(i16),
}

/// The synthesizer's state between samples.
struct Synth {
    rate: u32,
    phase: u32,
    /// What last sounded (kept while fading out): a tone or a sample.
    last: Out,
    /// 0..=RAMP: how far faded in.
    gain: i32,
}

impl Synth {
    /// The next sample: `out`, faded in from silence and back out to it.
    fn next(&mut self, out: Out) -> i16 {
        if let Out::Quiet = out {
            self.gain = (self.gain - 1).max(0);
        } else {
            // A new note keeps the phase, so changing pitch doesn't click.
            self.last = out;
            self.gain = (self.gain + 1).min(RAMP);
        }
        if self.gain == 0 {
            return 0;
        }
        match self.last {
            Out::Tone(hz) => {
                // A full turn of the phase is one cycle.
                self.phase = self.phase.wrapping_add((((hz as u64) << 32) / self.rate as u64) as u32);
                let level = AMPLITUDE * self.gain / RAMP;
                (if self.phase < 1 << 31 { level } else { -level }) as i16
            }
            Out::Sample(s) => (s as i32 * self.gain / RAMP) as i16,
            Out::Quiet => 0,
        }
    }
}

impl Voice {
    /// What should be sounding for the next sample, moving sounds along.
    fn advance(&mut self, rate: u32) -> Out {
        let samples = |note: &Note| (note.ms as u64 * rate as u64 / 1000).max(1) as u32;
        match &mut self.sound {
            Sound::Silent => Out::Quiet,
            Sound::Tone(hz) => Out::Tone(*hz),
            Sound::Samples { data, at } => match data.get(*at) {
                Some(&sample) => {
                    *at += 1;
                    Out::Sample(sample)
                }
                None => self.finish(),
            },
            Sound::Song { notes, at, left } => {
                if *left == 0 {
                    let Some(next) = notes.get(*at) else { return self.finish() };
                    *left = samples(next);
                    *at += 1;
                }
                let note = notes[*at - 1];
                *left -= 1;
                // The last eighth of each note is quiet, so repeated notes
                // come out as separate notes.
                if *left < samples(&note) / 8 || note.hz == 0 { Out::Quiet } else { Out::Tone(note.hz) }
            }
        }
    }

    /// Ends the sound playing.
    fn finish(&mut self) -> Out {
        self.sound = Sound::Silent;
        self.owner = None;
        Out::Quiet
    }
}

/// Starts a waiting desktop sound, if the voice is free.
fn take_ui_request() {
    let request = UI_REQUEST.swap(UI_NONE, Ordering::Relaxed);
    let notes = match request {
        UI_BEEP => vec![Note { hz: 880, ms: 80 }],
        UI_BONK => vec![Note { hz: 196, ms: 90 }, Note { hz: 131, ms: 160 }],
        _ => return,
    };
    let mut voice = VOICE.lock();
    if matches!(voice.sound, Sound::Silent) {
        voice.sound = song(notes);
        voice.owner = None;
        voice.id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    }
}

extern "C" fn sound_task() -> ! {
    let rate = snd::with(|s| s.rate).unwrap_or(48000);
    let mut synth = Synth { rate, phase: 0, last: Out::Quiet, gain: 0 };
    loop {
        take_ui_request();
        snd::with(|dev| {
            while let Some(i) = dev.reap() {
                let channels = dev.channels as usize;
                let samples = dev.samples(i);
                let mut voice = VOICE.lock();
                for frame in samples.chunks_exact_mut(channels) {
                    frame.fill(synth.next(voice.advance(rate)));
                }
                drop(voice);
                dev.submit(i);
            }
        });
        scheduler::wait::sleep_ms(snd::PERIOD_MS as u64);
    }
}

/// Starts the device and the task that feeds it.
pub fn init() {
    match snd::init() {
        Ok(()) => {
            scheduler::spawn("sound", sound_task);
        }
        Err(e) => log::info!("sound: {}", e),
    }
}
