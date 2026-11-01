//! WAV files: uncompressed PCM (8, 16 or 24-bit, any number of channels,
//! any common rate), decoded up front into what the voice plays: mono
//! 16-bit samples at the device's rate.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Longest sound we'll decode: three minutes at 48 kHz is about 17 MB.
pub const MAX_SECONDS: usize = 180;

const FORMAT_PCM: u16 = 1;
/// WAVE_FORMAT_EXTENSIBLE: the real format is in the sub-format GUID,
/// whose first two bytes are a plain format code.
const FORMAT_EXTENSIBLE: u16 = 0xfffe;

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

struct Format {
    channels: usize,
    rate: u32,
    bits: u16,
}

/// The `fmt ` chunk's format and the `data` chunk's bytes.
fn chunks(bytes: &[u8]) -> Result<(Format, &[u8]), String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(String::from("not a WAV file"));
    }
    let mut format = None;
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let len = u32_at(bytes, at + 4) as usize;
        let body = at + 8;
        // Recorders that stop without finishing the file leave the data
        // chunk's size 0 (or too big): take what's there.
        let end = if id == b"data" && (len == 0 || body + len > bytes.len()) { bytes.len() } else { body + len };
        if end > bytes.len() {
            return Err(format!("the '{}' chunk is cut short", String::from_utf8_lossy(id)));
        }
        let chunk = &bytes[body..end];
        match id {
            b"fmt " => {
                if chunk.len() < 16 {
                    return Err(String::from("format chunk too short"));
                }
                let mut code = u16_at(chunk, 0);
                if code == FORMAT_EXTENSIBLE && chunk.len() >= 26 {
                    code = u16_at(chunk, 24);
                }
                if code != FORMAT_PCM {
                    return Err(format!("only uncompressed PCM is supported (this is format {:#x})", code));
                }
                format = Some(Format {
                    channels: u16_at(chunk, 2) as usize,
                    rate: u32_at(chunk, 4),
                    bits: u16_at(chunk, 14),
                });
            }
            b"data" => {
                let format = format.ok_or("the data comes before the format")?;
                return Ok((format, chunk));
            }
            _ => {}
        }
        // Chunks are padded to an even length.
        at = end + (len & 1);
    }
    Err(String::from("no sound data in the file"))
}

/// Decodes a WAV file into mono samples at `rate` Hz.
pub fn decode(bytes: &[u8], rate: u32) -> Result<Vec<i16>, String> {
    let (format, data) = chunks(bytes)?;
    let Format { channels, rate: from, bits } = format;
    if !(1..=8).contains(&channels) {
        return Err(format!("{} channels isn't supported", channels));
    }
    if !(8000..=192_000).contains(&from) {
        return Err(format!("a sample rate of {} Hz isn't supported", from));
    }
    let width = match bits {
        8 | 16 | 24 => bits as usize / 8,
        _ => return Err(format!("{}-bit samples aren't supported (8, 16 or 24 are)", bits)),
    };
    let frames = data.len() / (width * channels);
    if frames / from as usize > MAX_SECONDS {
        return Err(format!("too long: {} s (at most {} s)", frames / from as usize, MAX_SECONDS));
    }

    // Each frame's channels averaged, as 16-bit.
    let sample = |at: usize| -> i32 {
        match width {
            1 => (data[at] as i32 - 128) << 8,
            2 => i16::from_le_bytes([data[at], data[at + 1]]) as i32,
            _ => i16::from_le_bytes([data[at + 1], data[at + 2]]) as i32,
        }
    };
    let mono = |frame: usize| -> i32 {
        let base = frame * width * channels;
        (0..channels).map(|c| sample(base + c * width)).sum::<i32>() / channels as i32
    };

    // Resample with linear interpolation, stepping through the source in
    // 32.32 fixed point.
    let out_len = (frames as u64 * rate as u64 / from as u64) as usize;
    let step = ((from as u64) << 32) / rate as u64;
    let mut out = Vec::new();
    out.try_reserve_exact(out_len).map_err(|_| String::from("not enough memory"))?;
    let mut pos: u64 = 0;
    for _ in 0..out_len {
        let i = (pos >> 32) as usize;
        let frac = (pos & 0xffff_ffff) as i64;
        let a = mono(i) as i64;
        let b = if i + 1 < frames { mono(i + 1) as i64 } else { a };
        out.push((a + ((b - a) * frac >> 32)) as i16);
        pos += step;
    }
    Ok(out)
}

#[cfg(feature = "test")]
pub mod tests {
    use super::*;
    use alloc::vec;

    /// A WAV file of `frames` (one Vec of channel values each), as `bits`-bit PCM.
    pub fn wav(rate: u32, bits: u16, frames: &[Vec<i16>]) -> Vec<u8> {
        let channels = frames.first().map_or(1, |f| f.len()) as u16;
        let width = bits as usize / 8;
        let mut data = Vec::new();
        for frame in frames {
            for &s in frame {
                match width {
                    1 => data.push(((s >> 8) + 128) as u8),
                    2 => data.extend_from_slice(&s.to_le_bytes()),
                    _ => { data.push(0); data.extend_from_slice(&s.to_le_bytes()); }
                }
            }
        }
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&FORMAT_PCM.to_le_bytes());
        b.extend_from_slice(&channels.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * channels as u32 * width as u32).to_le_bytes());
        b.extend_from_slice(&(channels * width as u16).to_le_bytes());
        b.extend_from_slice(&bits.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(data.len() as u32).to_le_bytes());
        b.extend_from_slice(&data);
        b
    }

    fn test_same_rate_mono_is_unchanged() {
        let frames: Vec<Vec<i16>> = [0, 1000, -1000, 32767, -32768].iter().map(|&s| vec![s]).collect();
        assert_eq!(decode(&wav(48000, 16, &frames), 48000).unwrap(), [0, 1000, -1000, 32767, -32768]);
    }

    fn test_stereo_is_averaged() {
        let frames = vec![vec![1000, 3000], vec![-2000, 0]];
        assert_eq!(decode(&wav(48000, 16, &frames), 48000).unwrap(), [2000, -1000]);
    }

    fn test_8_and_24_bit() {
        let frames = vec![vec![0x4000], vec![-0x4000]];
        assert_eq!(decode(&wav(48000, 8, &frames), 48000).unwrap(), [0x4000, -0x4000]);
        assert_eq!(decode(&wav(48000, 24, &frames), 48000).unwrap(), [0x4000, -0x4000]);
    }

    fn test_resampling_keeps_the_length_in_time() {
        // One second at 44.1 kHz is one second at 48 kHz, and a ramp stays a ramp.
        let frames: Vec<Vec<i16>> = (0..44100).map(|i| vec![(i % 1000) as i16]).collect();
        let out = decode(&wav(44100, 16, &frames), 48000).unwrap();
        assert_eq!(out.len(), 48000);
        assert_eq!(decode(&wav(24000, 16, &frames[..2400]), 48000).unwrap().len(), 4800);
        // Upsampling 2x puts the midpoint between neighbours.
        let two = vec![vec![0], vec![1000]];
        assert_eq!(decode(&wav(24000, 16, &two), 48000).unwrap(), [0, 500, 1000, 1000]);
    }

    fn test_chunks_before_data_are_skipped() {
        let mut b = wav(48000, 16, &[vec![7], vec![8]]);
        // A LIST chunk of odd length (so padded) between fmt and data.
        let list = [b"LIST".as_slice(), &3u32.to_le_bytes(), b"abc\0"].concat();
        b.splice(36..36, list);
        assert_eq!(decode(&b, 48000).unwrap(), [7, 8]);
    }

    fn test_unfinished_recording_still_plays() {
        // A data size of 0, as QEMU's recorder leaves it when killed.
        let mut b = wav(48000, 16, &[vec![5], vec![6]]);
        b[40..44].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(decode(&b, 48000).unwrap(), [5, 6]);
    }

    fn test_bad_files() {
        assert!(decode(b"hello", 48000).unwrap_err().contains("not a WAV"));
        let mut float = wav(48000, 16, &[vec![0]]);
        float[20] = 3; // IEEE float
        assert!(decode(&float, 48000).unwrap_err().contains("PCM"));
        let mut twelve = wav(48000, 16, &[vec![0]]);
        twelve[34] = 12;
        assert!(decode(&twelve, 48000).is_err());
        let no_data = &wav(48000, 16, &[vec![0]])[..36];
        assert!(decode(no_data, 48000).unwrap_err().contains("no sound data"));
    }

    pub fn run() {
        crate::test::run_all(&[
            &test_same_rate_mono_is_unchanged,
            &test_stereo_is_averaged,
            &test_8_and_24_bit,
            &test_resampling_keeps_the_length_in_time,
            &test_chunks_before_data_are_skipped,
            &test_unfinished_recording_still_plays,
            &test_bad_files,
        ]);
    }
}
