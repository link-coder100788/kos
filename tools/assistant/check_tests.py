"""Checks tests.txt: every line must name a real intent and only use slots
that intent has. Prints how many tests each intent has.

A line can also be a pattern, with `<slot>` where a value goes:
    ping | is <host> up
It's tested once for each of a few values that never appear in the
training data, which shows whether the model picks out details it hasn't
seen before.

    .venv/bin/python check_tests.py [tests.txt]
"""

import re
import sys
from collections import Counter

from intents import INTENTS
from kaitok import MARK, parse_markup, token_tags


# Values for `<slot>` patterns, deliberately absent from gen_data.py.
TEST_VALUES = {
    "host": ["openbsd.org", "172.16.0.9", "mastodon.social"],
    "site": ["openbsd.org", "python docs", "https://lobste.rs/"],
    "path": ["/files/music", "backups", "../old"],
    "topic": ["the usb stack", "sound", "the clock"],
}
# Other names people write for a slot.
SLOT_ALIASES = {"url": "site", "dir": "path", "folder": "path", "file": "path", "ip": "host"}
PATTERN = re.compile(r"<(\w+)>")


def expand(marked):
    """A pattern line's tests, one per test value; other lines as they are."""
    names = PATTERN.findall(marked)
    if not names:
        return [marked]
    out = []
    for i in range(3):
        line = marked
        for name in names:
            slot = SLOT_ALIASES.get(name, name)
            values = TEST_VALUES.get(slot, ["something"])
            line = line.replace(f"<{name}>", f"[{slot}: {values[i % len(values)]}]", 1)
        out.append(line)
    return out


def load(path):
    """(line number, intent, marked-up text) for each test in `path`, with
    pattern lines expanded."""
    tests = []
    for n, line in enumerate(open(path), 1):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        intent, sep, marked = line.partition("|")
        if not sep:
            raise SystemExit(f"{path}:{n}: expected 'intent | text'")
        tests += [(n, intent.strip(), m) for m in expand(marked.strip())]
    return tests


def main(path="tests.txt"):
    errors = 0
    counts = Counter()
    for n, intent, marked in load(path):
        if intent not in INTENTS:
            print(f"{path}:{n}: unknown intent '{intent}' (choose from {', '.join(INTENTS)})")
            errors += 1
            continue
        if marked.count("[") != marked.count("]") or len(MARK.findall(marked)) != marked.count("["):
            print(f"{path}:{n}: unbalanced or malformed [slot: value] markup")
            errors += 1
            continue
        text, spans = parse_markup(marked)
        for name, _, _ in spans:
            if name not in INTENTS[intent]:
                print(f"{path}:{n}: '{intent}' has no slot '{name}' (it has: {INTENTS[intent] or 'none'})")
                errors += 1
        if not token_tags(text, spans)[0] and intent != "help":
            # Empty input never reaches the model: K-AI shows its help.
            print(f"{path}:{n}: empty text (only 'help' can be empty)")
            errors += 1
        counts[intent] += 1
    for intent in INTENTS:
        print(f"{intent:15} {counts[intent]:3} tests")
    if errors:
        raise SystemExit(f"{errors} problem(s)")
    print("ok")


if __name__ == "__main__":
    main(*sys.argv[1:])
