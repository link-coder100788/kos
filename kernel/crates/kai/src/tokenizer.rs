//! The tokenizer and token features: an exact copy of
//! `tools/assistant/kaitok.py`, which documents the rules. If the two ever
//! disagree, the golden test fails.

use alloc::vec::Vec;

/// Hash buckets for token features (the embedding table's rows).
pub const BUCKETS: usize = 8192;
/// Tokens beyond this are ignored.
pub const MAX_TOKENS: usize = 32;

const LEADING: &[u8] = b"\"'([{";
const TRAILING: &[u8] = b"?!,;:\"')]}.";

/// A token, as a byte range of the text it came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub start: usize,
    pub end: usize,
}

/// Python's `str.split()` whitespace, in ASCII: space, \t \n \v \f \r.
fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

fn has_alnum(s: &[u8]) -> bool {
    s.iter().any(|b| b.is_ascii_alphanumeric())
}

/// Splits `text` into tokens. Lowercasing only changes ASCII letters, so
/// the byte ranges are the same in the original and lowercased text: slot
/// values are cut from the original, keeping their capitals.
pub fn tokenize(text: &str) -> Vec<Token> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if is_space(bytes[i]) {
            i += 1;
            continue;
        }
        let (mut start, mut end) = (i, i);
        while end < bytes.len() && !is_space(bytes[end]) {
            end += 1;
        }
        i = end;
        // Peel punctuation off the ends while a letter or digit remains.
        // Every byte peeled is ASCII, so the cuts stay on char boundaries.
        let mut tail = Vec::new();
        while end - start > 1 && LEADING.contains(&bytes[start]) && has_alnum(&bytes[start + 1..end]) {
            out.push(Token { start, end: start + 1 });
            start += 1;
        }
        while end - start > 1 && TRAILING.contains(&bytes[end - 1]) && has_alnum(&bytes[start..end - 1]) {
            tail.push(Token { start: end - 1, end });
            end -= 1;
        }
        out.push(Token { start, end });
        out.extend(tail.into_iter().rev());
    }
    out
}

/// 32-bit FNV-1a over the concatenation of `parts`.
fn fnv1a(parts: &[&[u8]]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for part in parts {
        for &b in *part {
            h ^= b as u32;
            h = h.wrapping_mul(0x0100_0193);
        }
    }
    h
}

/// The hashed bucket IDs for one (lowercased) token: the whole token,
/// then each 3-byte window of "<token>".
pub fn features(token: &[u8], out: &mut Vec<usize>) {
    out.clear();
    out.push(fnv1a(&[b"w:", token]) as usize % BUCKETS);
    let mut padded = Vec::with_capacity(token.len() + 2);
    padded.push(b'<');
    padded.extend_from_slice(token);
    padded.push(b'>');
    for window in padded.windows(3) {
        out.push(fnv1a(&[b"t:", window]) as usize % BUCKETS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::vec;

    fn words(text: &str) -> Vec<&str> {
        tokenize(text).iter().map(|t| &text[t.start..t.end]).collect()
    }

    #[test]
    fn peels_punctuation_but_keeps_paths_and_dots() {
        assert_eq!(words("Can you ping example.com?"), vec!["Can", "you", "ping", "example.com", "?"]);
        assert_eq!(words("cd .. / notes.txt."), vec!["cd", "..", "/", "notes.txt", "."]);
        assert_eq!(words("hey k-ai, open (wikipedia)!"), vec!["hey", "k-ai", ",", "open", "(", "wikipedia", ")", "!"]);
        assert!(tokenize(" \t\n").is_empty());
    }

    #[test]
    fn features_match_python() {
        // From `features('ping')` in kaitok.py.
        let mut ids = Vec::new();
        features(b"ping", &mut ids);
        assert_eq!(ids, vec![2876, 6354, 6674, 2877, 5842]);
    }
}
