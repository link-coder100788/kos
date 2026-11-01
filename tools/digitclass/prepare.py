"""Turns a drawing into the network's input: what MNIST digits look like.

KOS has an exact Rust copy of this, run on Paint's canvas, so every step is
plain arithmetic with an obvious counterpart there (no library resizing).

    ink (any size, 0 = paper, 1 = ink)
      -> crop to the ink
      -> thicken thin strokes to MNIST's proportions (Paint's brush is 4 px
         whatever the size of the digit; MNIST strokes are about a tenth of
         the digit's height)
      -> shrink (or grow) to fit a 20x20 box, averaging areas
      -> place in 28x28 with its centre of mass in the middle

Training images go through it too, so the network only ever sees what
this produces.
"""

import math

import numpy as np

SIZE = 28
BOX = 20
# Stroke width as a fraction of the digit's longer side: the median over
# MNIST's training digits (measured by `python prepare.py`).
STROKE = 0.142


def stroke_width(mask):
    """Average stroke width of a boolean mask: for a stroke of width w and
    length L, the area is about w*L and the edge pixels about 2*L."""
    area = int(mask.sum())
    p = np.pad(mask, 1)
    inner = p[1:-1, 1:-1] & p[:-2, 1:-1] & p[2:, 1:-1] & p[1:-1, :-2] & p[1:-1, 2:]
    edge = area - int(inner.sum())
    return 2.0 * area / edge if edge else 0.0


def grow(img, r):
    """Each pixel becomes the maximum of the (2r+1)x(2r+1) square around it
    (rows, then columns). The image keeps its size: pad it first."""
    h, w = img.shape
    p = np.pad(img, r)
    rows = p[:, 0:w].copy()
    for dx in range(1, 2 * r + 1):
        rows = np.maximum(rows, p[:, dx:dx + w])
    out = rows[0:h, :].copy()
    for dy in range(1, 2 * r + 1):
        out = np.maximum(out, rows[dy:dy + h, :])
    return out


def area_matrix(n_out, n_in):
    """[n_out, n_in] weights: output pixel i averages input pixels over
    [i, i+1) * n_in / n_out, each weighted by how much of it is covered."""
    m = np.zeros((n_out, n_in), dtype=np.float32)
    step = n_in / n_out
    for i in range(n_out):
        lo, hi = i * step, (i + 1) * step
        for j in range(int(math.floor(lo)), min(n_in, int(math.ceil(hi)))):
            m[i, j] = (min(hi, j + 1) - max(lo, j)) / step
    return m


def round_half_up(x):
    return int(math.floor(x + 0.5))


def prepare(ink):
    """ink: [h, w] float32 in [0, 1]. Returns [28, 28] float32 in [0, 1],
    all zeros if there's no ink."""
    ink = np.asarray(ink, dtype=np.float32)
    mask = ink >= 0.5
    out = np.zeros((SIZE, SIZE), dtype=np.float32)
    if not mask.any():
        return out
    rows, cols = np.flatnonzero(mask.any(axis=1)), np.flatnonzero(mask.any(axis=0))
    top, bottom, left, right = rows[0], rows[-1] + 1, cols[0], cols[-1] + 1
    ink, mask = ink[top:bottom, left:right], mask[top:bottom, left:right]

    longer = max(ink.shape)
    r = round_half_up((STROKE * longer - stroke_width(mask)) / 2)
    if r > 0:
        ink = grow(np.pad(ink, r), r)

    h, w = ink.shape
    scale = BOX / max(h, w)
    oh, ow = max(1, round_half_up(h * scale)), max(1, round_half_up(w * scale))
    small = area_matrix(oh, h) @ ink @ area_matrix(ow, w).T

    total = small.sum()
    cy = float((small.sum(axis=1) * (np.arange(oh) + 0.5)).sum() / total)
    cx = float((small.sum(axis=0) * (np.arange(ow) + 0.5)).sum() / total)
    y0 = min(max(round_half_up(SIZE / 2 - cy), 0), SIZE - oh)
    x0 = min(max(round_half_up(SIZE / 2 - cx), 0), SIZE - ow)
    out[y0:y0 + oh, x0:x0 + ow] = np.clip(small, 0.0, 1.0)
    return out


if __name__ == "__main__":
    # Measures STROKE from MNIST's training digits.
    from data import load_mnist
    images, _ = load_mnist("train")
    ratios = []
    for img in images[:10000]:
        mask = img >= 128
        rows, cols = np.flatnonzero(mask.any(axis=1)), np.flatnonzero(mask.any(axis=0))
        crop = mask[rows[0]:rows[-1] + 1, cols[0]:cols[-1] + 1]
        ratios.append(stroke_width(crop) / max(crop.shape))
    print(f"stroke / longer side: median {np.median(ratios):.3f}, "
          f"10% {np.percentile(ratios, 10):.3f}, 90% {np.percentile(ratios, 90):.3f}")
