//! The one-voice sound system: its notation, and playing through the
//! device (`make test` records it to build/test.wav).

use core::sync::atomic::Ordering;
use crate::arch::timer::uptime_ms;
use crate::drivers::virtio::snd::PERIODS_PLAYED;
use crate::scheduler::wait::sleep_ms;
use alloc::vec::Vec;
use crate::filesys::fs_ipc::{fs_call, FsRequest, FsResponse};
use crate::sound::{self, parse, wav, Note};

fn note(hz: u32, ms: u32) -> Note {
    Note { hz, ms }
}

/// Waits up to `ms` for `f` to hold.
fn within(ms: u64, f: impl Fn() -> bool) -> bool {
    let start = uptime_ms();
    while !f() {
        if uptime_ms() - start > ms {
            return false;
        }
        sleep_ms(10);
    }
    true
}

fn test_notes_and_octaves() {
    // A4 is 440 Hz, a quarter note at 120 a minute is 500 ms.
    assert_eq!(parse("A").unwrap(), [note(440, 500)]);
    assert_eq!(parse("o4 c").unwrap(), [note(262, 500)]);
    assert_eq!(parse("A > A < < A").unwrap(), [note(440, 500), note(880, 500), note(220, 500)]);
    assert_eq!(parse("O5 A O3 A").unwrap(), [note(880, 500), note(220, 500)]);
    // Sharps and flats: C#4 and D-4 are the same key.
    assert_eq!(parse("C# C+ D-").unwrap(), [note(277, 500); 3]);
    assert_eq!(parse("B- A#").unwrap(), [note(466, 500); 2]);
}

fn test_lengths_and_tempo() {
    assert_eq!(parse("C8 C2 C1").unwrap().iter().map(|n| n.ms).collect::<Vec<_>>(), [250, 1000, 2000]);
    assert_eq!(parse("C8.").unwrap()[0].ms, 375);
    assert_eq!(parse("C4..").unwrap()[0].ms, 875);
    assert_eq!(parse("T60 C").unwrap()[0].ms, 1000);
    assert_eq!(parse("L8 C D4").unwrap().iter().map(|n| n.ms).collect::<Vec<_>>(), [250, 500]);
    assert_eq!(parse("R2 P").unwrap(), [note(0, 1000), note(0, 500)]);
    assert!(parse("").unwrap().is_empty());
}

fn test_bad_notation() {
    assert!(parse("C X").unwrap_err().contains("character 3"));
    assert!(parse("T10").is_err());
    assert!(parse("O9 C").is_err());
    assert!(parse("C0").is_err());
    assert!(parse("O0 <").is_err());
}

fn test_tone_plays_and_stops() {
    assert!(sound::available(), "no sound device (the Makefile should attach one)");
    let before = PERIODS_PLAYED.load(Ordering::Relaxed);
    sound::snd(440).unwrap();
    assert!(sound::busy());
    sleep_ms(200);
    // The device plays a 10 ms period at a time, at the sample rate.
    let played = PERIODS_PLAYED.load(Ordering::Relaxed) - before;
    assert!(played >= 10, "only {} periods played in 200 ms", played);
    sound::snd(0).unwrap();
    assert!(!sound::busy());
}

fn test_song_finishes() {
    // Two 62 ms notes.
    let id = sound::play("T240 C16 D16").unwrap();
    assert!(sound::busy());
    assert!(!sound::finished(id));
    assert!(within(1000, || sound::finished(id) && !sound::busy()), "the song never finished");
}

fn test_new_sound_replaces_old() {
    let song = sound::play("C1 C1 C1").unwrap();
    sound::snd(300).unwrap();
    assert!(sound::finished(song));
    sleep_ms(50);
    // The tone replaced the song, and stopping it leaves nothing playing.
    sound::stop().unwrap();
    assert!(!sound::busy());
}

fn test_ui_beep_waits_for_quiet() {
    sound::ui_beep();
    assert!(within(500, sound::busy), "the beep never started");
    assert!(within(1000, || !sound::busy()), "the beep never ended");
}

fn test_wav_plays_and_finishes() {
    // A quarter second of a 22.05 kHz stereo sawtooth.
    let frames: Vec<Vec<i16>> = (0..5512).map(|i| { let s = ((i % 100) * 300 - 15000) as i16; alloc::vec![s, s] }).collect();
    let before = PERIODS_PLAYED.load(Ordering::Relaxed);
    let id = sound::play_wav(&wav::tests::wav(22050, 16, &frames)).unwrap();
    assert!(!sound::finished(id));
    assert!(within(2000, || sound::finished(id)), "the WAV never finished");
    assert!(PERIODS_PLAYED.load(Ordering::Relaxed) - before >= 20);
    assert!(sound::play_wav(b"not a wav").is_err());
}

fn test_wav_file_from_the_disk() {
    let FsResponse::Data(bytes) = fs_call(FsRequest::Read { cwd: "/".into(), path: "sounds/chime.wav".into() }) else {
        panic!("no /sounds/chime.wav on the disk");
    };
    // 1.8 s at 44.1 kHz stereo, at the device's rate in mono.
    let samples = wav::decode(&bytes, sound::rate()).unwrap();
    assert_eq!(samples.len(), (sound::rate() as usize * 18) / 10);
    assert!(samples.iter().any(|&s| s.unsigned_abs() > 3000), "the chime is silent");
    let id = sound::play_file("/", "sounds/chime.wav").unwrap();
    sleep_ms(100);
    // Another sound cuts it off.
    sound::beep().unwrap();
    assert!(sound::finished(id));
    assert!(within(1000, || !sound::busy()));
    assert!(sound::play_file("/", "sounds/missing.wav").is_err());
}

pub fn run() {
    crate::test::run_all(&[
        &test_notes_and_octaves,
        &test_lengths_and_tempo,
        &test_bad_notation,
        &test_tone_plays_and_stops,
        &test_song_finishes,
        &test_new_sound_replaces_old,
        &test_ui_beep_waits_for_quiet,
        &test_wav_plays_and_finishes,
        &test_wav_file_from_the_disk,
    ]);
}
