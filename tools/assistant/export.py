"""Exports the trained model for KOS, plus golden outputs to check the Rust
copy against.

    .venv/bin/python export.py      # data/kai.pt -> ../../disk/kai/kai.kosm, golden.txt

The model file (all little-endian):

    "KOSM"  u32 version (1)
    u32 buckets, max_tokens, d, layers, heads, ff
    u32 intent count, then each name as u8 length + UTF-8
    u32 tag count, then each name the same way
    f32 tensors, row-major, Linear weights as [out, in]:
        embed [buckets, d]           (without the padding row)
        position [max_tokens, d]
        per block: norm1 w, b [d]; q, k, v, o w [d, d], b [d];
                   norm2 w, b [d]; ff1 w [ff, d], b [ff]; ff2 w [d, ff], b [d]
        norm w, b [d]; intent w [intents, d], b; tag w [tags, d], b

LayerNorm's epsilon is PyTorch's default, 1e-5.
"""

import json
import struct
from pathlib import Path

import torch

from intents import INTENT_NAMES, TAGS
from kaitok import BUCKETS, MAX_TOKENS, tokenize
from model import Config, KaiModel
from train import encode

OUT = Path(__file__).resolve().parents[2] / "disk" / "kai" / "kai.kosm"

# Sentences for golden.txt: a spread of intents, slots, casing, typos and
# punctuation, so the Rust tokenizer and maths are both exercised.
GOLDEN = [
    "what can you do", "Whats running rn?", "how much mem is left", "uptime", "am i connected",
    "can you ping debian.org", "is 10.0.2.2 up?", "Open https://en.wikipedia.org/wiki/Kernel_(operating_system)",
    "take me to hacker news", "whats in /files/files2/images", "ls ..", "what's the weather like",
    "tell me a joke!", "K-AI, show memory usage please", "pnig exmaple.com", "list backups",
    "hey kai, (open) wikipedia!!", "\"memory\"", "a b c d e f g h i j k l m n o p q r s t u v w x y z 1 2 3 4 5 6 7 8 9",
    "net stats", "please delete Notes/Old.TXT", "what time is it?",
    "what went wrong", "why isn't the network working?",
]


def names(f, items):
    f.write(struct.pack("<I", len(items)))
    for item in items:
        data = item.encode()
        f.write(struct.pack("<B", len(data)) + data)


def tensor(f, t):
    f.write(t.detach().to(torch.float32).contiguous().numpy().astype("<f4").tobytes())


def main():
    c = Config(len(INTENT_NAMES), len(TAGS))
    model = KaiModel(c)
    model.load_state_dict(torch.load("data/kai.pt"))
    model.eval()

    OUT.parent.mkdir(parents=True, exist_ok=True)
    with open(OUT, "wb") as f:
        f.write(b"KOSM" + struct.pack("<I", 1))
        f.write(struct.pack("<6I", BUCKETS, MAX_TOKENS, c.d, c.layers, c.heads, c.ff))
        names(f, INTENT_NAMES)
        names(f, TAGS)
        tensor(f, model.embed.weight[:BUCKETS])
        tensor(f, model.position.weight)
        for b in model.blocks:
            for layer in [b.norm1, b.q, b.k, b.v, b.o, b.norm2, b.ff1, b.ff2]:
                tensor(f, layer.weight)
                tensor(f, layer.bias)
        for layer in [model.norm, model.intent, model.tag]:
            tensor(f, layer.weight)
            tensor(f, layer.bias)
    print(f"wrote {OUT} ({OUT.stat().st_size:,} bytes)")

    # One sentence at a time, as KOS will run it: no batch padding.
    with torch.no_grad(), open("golden.txt", "w") as f:
        f.write("# K-AI golden outputs from export.py: text, tokens, intent scores, then\n"
                "# one line of tag scores per token. KOS's copy must match these closely.\n")
        for text in GOLDEN:
            tokens = [t for t, _, _ in tokenize(text)][:MAX_TOKENS]
            intents, tags = model(*encode([tokens]))
            f.write(f"text\t{text}\n")
            f.write("tokens\t" + json.dumps(tokens) + "\n")
            f.write("intent\t" + " ".join(f"{v:.6e}" for v in intents[0].tolist()) + "\n")
            for row in tags[0].tolist():
                f.write("tag\t" + " ".join(f"{v:.6e}" for v in row) + "\n")
            f.write("\n")
    print(f"wrote golden.txt ({len(GOLDEN)} sentences)")


if __name__ == "__main__":
    main()
