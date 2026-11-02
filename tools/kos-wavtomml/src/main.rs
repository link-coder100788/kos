use hound::WavReader;
use std::env;

const MIN_FREQ: f32 = 65.41;  // C2
const MAX_FREQ: f32 = 1046.50; // C6
const NOTE_NAMES: [&str; 12] = ["c", "c#", "d", "d#", "e", "f", "f#", "g", "g#", "a", "a#", "b"];

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <path_to_wav>", args[0]);
        std::process::exit(1);
    }

    let mut reader = WavReader::open(&args[1]).expect("Failed to open WAV file");
    let spec = reader.spec();
    let sample_rate = spec.sample_rate as f32;

    // 1. Decode and safely handle sample formats
    let raw_samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => {
            let max_val = (1u64 << (spec.bits_per_sample - 1)) as f32;
            reader.samples::<i32>().map(|s| s.unwrap() as f32 / max_val).collect()
        }
        hound::SampleFormat::Float => reader.samples::<f32>().map(|s| s.unwrap()).collect(),
    };

    // Downmix Stereo safely (using basic channel separation tracking)
    let mono_samples = if spec.channels == 2 {
        raw_samples.chunks_exact(2).map(|ch| ch[0]).collect()
    } else {
        raw_samples
    };

    // 2. Dynamic Volume Normalization (Fixes low volume issue)
    let max_amplitude = mono_samples.iter().map(|&s| s.abs()).fold(0.0f32, f32::max);
    let normalized_samples: Vec<f32> = if max_amplitude > 0.0 {
        mono_samples.iter().map(|&s| s / max_amplitude).collect()
    } else {
        mono_samples
    };

    // 3. Process 50ms windows
    let window_size = (sample_rate * 0.05) as usize;
    let mut mml_output = String::new();
    let mut current_octave = 4;

    mml_output.push_str("T120 ");

    for chunk in normalized_samples.chunks_exact(window_size) {
        if let Some(freq) = detect_pitch_amdf(chunk, sample_rate) {
            if freq >= MIN_FREQ && freq <= MAX_FREQ {
                let (note, octave) = freq_to_note(freq);

                if octave != current_octave {
                    if octave > current_octave {
                        mml_output.push_str(&">".repeat((octave - current_octave) as usize));
                    } else {
                        mml_output.push_str(&"<".repeat((current_octave - octave) as usize));
                    }
                    current_octave = octave;
                }
                mml_output.push_str(&format!("{}16 ", note));
            } else {
                mml_output.push_str("r16 ");
            }
        } else {
            mml_output.push_str("r16 ");
        }
    }

    println!("\nGenerated MML:\n{}", mml_output.trim());
}

/// AMDF (Average Magnitude Difference Function) Pitch Detection
fn detect_pitch_amdf(signal: &[f32], sample_rate: f32) -> Option<f32> {
    // Highly permissive noise gate on normalized signals
    let rms: f32 = (signal.iter().map(|&x| x * x).sum::<f32>() / signal.len() as f32).sqrt();
    if rms < 0.005 { return None; }

    let max_period = (sample_rate / MIN_FREQ) as usize;
    let min_period = (sample_rate / MAX_FREQ) as usize;

    let mut best_period = 0;
    let mut min_diff = f32::MAX;

    // AMDF looks for the period valley where the wave overlaps perfectly with itself (difference near 0)
    for period in min_period..=max_period {
        if period >= signal.len() { break; }

        let mut diff = 0.0;
        for i in 0..(signal.len() - period) {
            diff += (signal[i] - signal[i + period]).abs();
        }
        diff /= (signal.len() - period) as f32;

        if diff < min_diff {
            min_diff = diff;
            best_period = period;
        }
    }

    // Sanity check: Ensure our valley isn't flat noise
    if best_period > 0 && min_diff < 0.5 {
        Some(sample_rate / best_period as f32)
    } else {
        None
    }
}

fn freq_to_note(freq: f32) -> (String, i32) {
    let midi_note = (12.0 * (freq / 440.0).log2() + 69.0).round() as i32;
    let note_index = (midi_note % 12).rem_euclid(12) as usize;
    let octave = (midi_note / 12) - 1;
    (NOTE_NAMES[note_index].to_string(), octave)
}
