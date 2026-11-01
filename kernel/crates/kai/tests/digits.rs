//! Checks the Rust digit recogniser against PyTorch: `tools/digitclass/
//! export.py` writes `golden.txt` with drawings, what `prepare.py` made of
//! them and the network's scores, and the Rust copy must match both.

use std::path::Path;

use kai::digits::{split, DigitModel, SIZE};

/// Sums happen in a different order here than in numpy and PyTorch, so
/// values differ in the last few bits; this is far tighter than any
/// decision.
const TOLERANCE: f32 = 1e-3;

struct Case {
    w: usize,
    h: usize,
    ink: Vec<f32>,
    prepared: Vec<f32>,
    scores: Vec<f32>,
}

fn repo() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(3).unwrap()
}

fn model() -> DigitModel {
    DigitModel::load(&std::fs::read(repo().join("disk/digits/digits.kosm")).unwrap()).unwrap()
}

fn floats(s: &str) -> Vec<f32> {
    s.split(' ').map(|v| v.parse().unwrap()).collect()
}

fn cases() -> Vec<Case> {
    let text = std::fs::read_to_string(repo().join("tools/digitclass/golden.txt")).unwrap();
    let mut cases: Vec<Case> = Vec::new();
    let mut row = 0;
    for line in text.lines().filter(|l| !l.starts_with('#') && !l.is_empty()) {
        let (kind, value) = line.split_once('\t').unwrap_or((line, ""));
        match kind {
            "drawing" => {
                let (h, w) = value.split_once(' ').unwrap();
                let (h, w) = (h.parse().unwrap(), w.parse().unwrap());
                cases.push(Case { w, h, ink: vec![0.0; w * h], prepared: Vec::new(), scores: Vec::new() });
                row = 0;
            }
            "row" => {
                let case = cases.last_mut().unwrap();
                for run in value.split(',').filter(|r| !r.is_empty()) {
                    let (a, b) = run.split_once('-').unwrap();
                    let (a, b): (usize, usize) = (a.parse().unwrap(), b.parse().unwrap());
                    case.ink[row * case.w + a..row * case.w + b].fill(1.0);
                }
                row += 1;
            }
            "prepared" => cases.last_mut().unwrap().prepared = floats(value),
            "scores" => cases.last_mut().unwrap().scores = floats(value),
            _ => panic!("unexpected line in golden.txt: {line}"),
        }
    }
    cases
}

fn worst(got: &[f32], want: &[f32]) -> f32 {
    assert_eq!(got.len(), want.len());
    got.iter().zip(want).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max)
}

#[test]
fn matches_pytorch() {
    let model = model();
    let cases = cases();
    assert!(cases.len() >= 10, "golden.txt has only {} drawings", cases.len());
    let (mut worst_prepared, mut worst_scores) = (0.0f32, 0.0f32);
    for (i, case) in cases.iter().enumerate() {
        let prepared = model.prepare(&case.ink, case.w, case.h);
        let d = worst(&prepared, &case.prepared);
        assert!(d <= TOLERANCE, "drawing {i} ({}x{}): prepared input differs from prepare.py by {d}", case.w, case.h);
        let scores = model.run(&case.prepared);
        let d2 = worst(&scores, &case.scores);
        assert!(d2 <= TOLERANCE, "drawing {i}: scores differ from PyTorch by {d2}");
        (worst_prepared, worst_scores) = (worst_prepared.max(d), worst_scores.max(d2));
    }
    println!("{} drawings match; largest differences: prepared {worst_prepared:e}, scores {worst_scores:e}", cases.len());
}

#[test]
fn reads_digits_side_by_side() {
    let model = model();
    let cases = cases();
    // The golden digits 0..9, drawn next to each other with a gap.
    let digits = &cases[..10];
    let h = digits.iter().map(|c| c.h).max().unwrap();
    let w: usize = digits.iter().map(|c| c.w + 10).sum();
    let mut ink = vec![0.0f32; w * h];
    let mut x0 = 0;
    for case in digits {
        for y in 0..case.h {
            ink[y * w + x0..y * w + x0 + case.w].copy_from_slice(&case.ink[y * case.w..(y + 1) * case.w]);
        }
        x0 += case.w + 10;
    }
    assert_eq!(split(&ink, w, h).len(), 10);
    let read: Vec<u8> = model.read_all(&ink, w, h).iter().map(|r| r.digit).collect();
    assert_eq!(read, (0..10).collect::<Vec<u8>>());
}

#[test]
fn reads_nothing_from_a_blank_canvas() {
    let model = model();
    assert!(model.read(&[0.0; 100], 10, 10).is_none());
    assert!(model.read_all(&[0.0; 100], 10, 10).is_empty());
    assert_eq!(model.prepare(&[0.0; 100], 10, 10), vec![0.0; SIZE * SIZE]);
}

#[test]
fn rejects_bad_files() {
    assert!(DigitModel::load(b"nope").is_err());
    let mut data = std::fs::read(repo().join("disk/digits/digits.kosm")).unwrap();
    data.truncate(data.len() - 1);
    assert!(DigitModel::load(&data).is_err());
}
