//! Checks the Rust model against PyTorch: `tools/assistant/export.py`
//! writes `golden.txt` with PyTorch's scores for a set of sentences, and
//! the Rust copy must give the same tokens and (nearly) the same scores.

use std::path::Path;

use kai::tokenizer::MAX_TOKENS;
use kai::Model;

/// Float sums happen in a different order here than in PyTorch, so scores
/// differ in the last few bits; this is far tighter than any decision.
const TOLERANCE: f32 = 1e-3;

struct Case {
    text: String,
    tokens: Vec<String>,
    intent: Vec<f32>,
    tags: Vec<Vec<f32>>,
}

fn repo() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(3).unwrap()
}

fn floats(s: &str) -> Vec<f32> {
    s.split(' ').map(|v| v.parse().unwrap()).collect()
}

/// The tokens line is a JSON list of strings, as Python's json.dumps
/// writes it: `["a", "\"", "b\\c"]`.
fn json_strings(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '"' {
            continue;
        }
        let mut token = String::new();
        loop {
            match chars.next().expect("unterminated string in golden.txt") {
                '"' => break,
                '\\' => match chars.next().unwrap() {
                    'n' => token.push('\n'),
                    't' => token.push('\t'),
                    'u' => {
                        let hex: String = chars.by_ref().take(4).collect();
                        token.push(char::from_u32(u32::from_str_radix(&hex, 16).unwrap()).unwrap());
                    }
                    other => token.push(other),
                },
                other => token.push(other),
            }
        }
        out.push(token);
    }
    out
}

fn cases() -> Vec<Case> {
    let text = std::fs::read_to_string(repo().join("tools/assistant/golden.txt")).unwrap();
    let mut cases = Vec::new();
    for line in text.lines().filter(|l| !l.starts_with('#') && !l.is_empty()) {
        let (kind, value) = line.split_once('\t').unwrap();
        match kind {
            "text" => cases.push(Case { text: value.to_string(), tokens: Vec::new(), intent: Vec::new(), tags: Vec::new() }),
            "tokens" => cases.last_mut().unwrap().tokens = json_strings(value),
            "intent" => cases.last_mut().unwrap().intent = floats(value),
            "tag" => cases.last_mut().unwrap().tags.push(floats(value)),
            _ => panic!("unexpected line in golden.txt: {line}"),
        }
    }
    cases
}

fn assert_close(what: &str, text: &str, got: &[f32], want: &[f32]) {
    assert_eq!(got.len(), want.len(), "{what} for {text:?}: wrong length");
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        assert!((g - w).abs() <= TOLERANCE, "{what}[{i}] for {text:?}: got {g}, PyTorch {w}");
    }
}

#[test]
fn matches_pytorch() {
    let model = Model::load(&std::fs::read(repo().join("disk/kai/kai.kosm")).unwrap()).unwrap();
    let cases = cases();
    assert!(cases.len() >= 20, "golden.txt has only {} sentences", cases.len());
    let mut worst = 0.0f32;
    for case in &cases {
        let out = model.run(&case.text);
        let lower = case.text.to_ascii_lowercase();
        let tokens: Vec<&str> = out.tokens.iter().map(|t| &lower[t.start..t.end]).collect();
        assert_eq!(tokens, case.tokens, "tokens for {:?}", case.text);
        assert!(out.tokens.len() <= MAX_TOKENS);
        assert_close("intent", &case.text, &out.intent_scores, &case.intent);
        assert_eq!(out.tag_scores.len(), case.tags.len(), "tag rows for {:?}", case.text);
        for (row, want) in out.tag_scores.iter().zip(&case.tags) {
            assert_close("tag", &case.text, row, want);
        }
        let rows = std::iter::once((&out.intent_scores, &case.intent)).chain(out.tag_scores.iter().zip(&case.tags));
        for (g, w) in rows {
            worst = g.iter().zip(w).map(|(a, b)| (a - b).abs()).fold(worst, f32::max);
        }
    }
    println!("{} sentences match PyTorch; largest difference {worst:e}", cases.len());
}

#[test]
fn understands_examples() {
    let model = Model::load(&std::fs::read(repo().join("disk/kai/kai.kosm")).unwrap()).unwrap();
    let u = model.understand("can you ping Debian.org?");
    assert_eq!(u.intent, "ping");
    assert_eq!(u.slots, vec![("host".to_string(), "Debian.org".to_string())]);
    let u = model.understand("Open https://en.wikipedia.org/wiki/Kernel_(operating_system)");
    assert_eq!(u.intent, "open_url");
    assert_eq!(u.slots, vec![("site".to_string(), "https://en.wikipedia.org/wiki/Kernel_(operating_system)".to_string())]);
}

#[test]
fn rejects_bad_files() {
    assert!(Model::load(b"nope").is_err());
    let mut data = std::fs::read(repo().join("disk/kai/kai.kosm")).unwrap();
    data.truncate(data.len() - 1);
    assert!(Model::load(&data).is_err());
}
