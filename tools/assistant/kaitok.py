"""K-AI's tokenizer and token features. KOS has an exact Rust copy (in
kernel/crates/kos-nlu); keep the two in step, and `golden.txt` will catch
it if they drift.

Tokens:
  1. ASCII letters are lowercased; nothing else changes.
  2. The text is split on ASCII whitespace.
  3. Each piece has punctuation peeled off its ends as tokens of their own
     (leading: " ' ( [ {   trailing: ? ! , ; : " ' ) ] } .), but only
     while what's left still has a letter or digit. So "example.com?"
     gives "example.com" and "?", while "..", "/" and "notes.txt" stay
     whole.

Features: a token is represented by hashed IDs (fastText style), so words
never seen in training, like file and site names, still get a sensible
embedding from their pieces:
  - the whole token, as "w:" + token
  - every 3-byte window of "<" + token + ">", as "t:" + window
each hashed with 32-bit FNV-1a over the UTF-8 bytes, modulo BUCKETS.
"""

import re

BUCKETS = 8192
MAX_TOKENS = 32
LEADING = "\"'([{"
TRAILING = "?!,;:\"')]}."


def lower_ascii(s):
    return "".join(chr(ord(c) + 32) if "A" <= c <= "Z" else c for c in s)


def _has_alnum(s):
    return any(c.isascii() and c.isalnum() for c in s)


def tokenize(text):
    """Tokens of `text`, as (token, start, end) with character offsets."""
    text = lower_ascii(text)
    out = []
    for m in re.finditer(r"[^ \t\n\r\f\v]+", text):
        start, end = m.start(), m.end()
        head, tail = [], []
        while end - start > 1 and text[start] in LEADING and _has_alnum(text[start + 1:end]):
            head.append((text[start], start, start + 1))
            start += 1
        while end - start > 1 and text[end - 1] in TRAILING and _has_alnum(text[start:end - 1]):
            tail.append((text[end - 1], end - 1, end))
            end -= 1
        out += head + [(text[start:end], start, end)] + tail[::-1]
    return out


def fnv1a(data: bytes) -> int:
    h = 0x811C9DC5
    for b in data:
        h ^= b
        h = (h * 0x01000193) & 0xFFFFFFFF
    return h


def features(token):
    """The hashed bucket IDs for one token."""
    ids = [fnv1a(b"w:" + token.encode()) % BUCKETS]
    padded = b"<" + token.encode() + b">"
    for i in range(len(padded) - 2):
        ids.append(fnv1a(b"t:" + padded[i:i + 3]) % BUCKETS)
    return ids


# ---- slot markup -------------------------------------------------------------
#
# Examples are written with slots marked inline:
#     can you ping [host: example.com] for me
# `parse_markup` turns that into the plain text and the character span of
# each slot, and `token_tags` into one tag per token.

MARK = re.compile(r"\[(\w+):\s*([^\]]*)\]")


def parse_markup(marked):
    """'ping [host: x.org]' -> ('ping x.org', [('host', 5, 10)])."""
    text, spans, pos = "", [], 0
    for m in MARK.finditer(marked):
        text += marked[pos:m.start()]
        value = m.group(2).strip()
        spans.append((m.group(1), len(text), len(text) + len(value)))
        text += value
        pos = m.end()
    return text + marked[pos:], spans


def token_tags(text, spans):
    """Tokens of `text` and their B-/I-/O tags from character `spans`."""
    tokens = tokenize(text)
    tags = []
    for _, start, end in tokens:
        tag = "O"
        for name, s, e in spans:
            if start >= s and end <= e:
                tag = ("B-" if start == s else "I-") + name
        tags.append(tag)
    return [t for t, _, _ in tokens], tags
