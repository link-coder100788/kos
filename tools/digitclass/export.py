"""Exports the trained recogniser for KOS, plus golden outputs to check the
Rust copy against.

    .venv/bin/python export.py      # data/digits.pt -> ../../disk/digits/digits.kosm, golden.txt

File format (little-endian): b"KOSD", u32 version 1, then u32 SIZE, BOX,
c1, c2, hidden, classes; f32 STROKE (so KOS prepares drawings exactly as
training did); then f32 tensors in PyTorch's order: conv1 weight
[c1, 1, 3, 3] and bias, conv2 weight [c2, c1, 3, 3] and bias, fc1 weight
[hidden, c2 * 7 * 7] (inputs ordered channel, row, column) and bias, fc2
weight [classes, hidden] and bias.
"""

import struct
from pathlib import Path

import numpy as np
import torch

from data import load_mnist, mouse
from model import Config, DigitModel
from prepare import BOX, SIZE, STROKE, prepare

OUT = Path(__file__).resolve().parents[2] / "disk" / "digits" / "digits.kosm"


def tensor(f, t):
    f.write(t.detach().to(torch.float32).contiguous().numpy().astype("<f4").tobytes())


def runs(row):
    """"3-7,12-13": the columns [start, end) that have ink."""
    ink = np.flatnonzero(np.diff(np.concatenate([[0], row.astype(np.int8), [0]])))
    return ",".join(f"{a}-{b}" for a, b in zip(ink[::2], ink[1::2]))


def golden_drawings():
    """Mouse-drawn digits of every kind and size, plus a speck (prepared by
    growing, not shrinking) and an empty canvas."""
    images, labels = load_mnist("test")
    rng = np.random.default_rng(7)
    drawings = [mouse(images[np.flatnonzero(labels == d)[0]], rng) for d in range(10)]
    speck = np.zeros((9, 9), dtype=np.float32)
    speck[3:6, 4:6] = 1
    return drawings + [speck, np.zeros((5, 5), dtype=np.float32)]


def main():
    c = Config()
    model = DigitModel(c)
    model.load_state_dict(torch.load("data/digits.pt"))
    model.eval()

    OUT.parent.mkdir(parents=True, exist_ok=True)
    with open(OUT, "wb") as f:
        f.write(b"KOSD" + struct.pack("<I", 1))
        f.write(struct.pack("<6I", SIZE, BOX, c.c1, c.c2, c.hidden, c.classes))
        f.write(struct.pack("<f", STROKE))
        for layer in [model.conv1, model.conv2, model.fc1, model.fc2]:
            tensor(f, layer.weight)
            tensor(f, layer.bias)
    print(f"wrote {OUT} ({OUT.stat().st_size:,} bytes)")

    with torch.no_grad(), open("golden.txt", "w") as f:
        f.write("# Digit recogniser golden outputs from export.py. Each drawing: its size,\n"
                "# one line per row giving the [start, end) columns with ink, the prepared\n"
                "# 28x28 input, then the scores. KOS's copy must match these closely.\n")
        drawings = golden_drawings()
        for ink in drawings:
            h, w = ink.shape
            f.write(f"drawing\t{h} {w}\n")
            for row in ink >= 0.5:
                f.write(f"row\t{runs(row)}\n")
            x = prepare(ink)
            f.write("prepared\t" + " ".join(f"{v:.6e}" for v in x.flatten().tolist()) + "\n")
            out = model(torch.from_numpy(x)[None, None])
            f.write("scores\t" + " ".join(f"{v:.6e}" for v in out[0].tolist()) + "\n\n")
    print(f"wrote golden.txt ({len(drawings)} drawings)")


if __name__ == "__main__":
    main()
