"""MNIST, and digits made to look drawn with Paint's mouse.

MNIST digits are pen strokes, scanned: soft, of varying width, and already
cropped and centred. Paint gives hard-edged strokes of one fixed width, at
any size and slant. So most training digits are redrawn: an MNIST digit is
blown up, slanted and rotated, thinned to a one-pixel skeleton, and drawn
over with a round brush, like a mouse tracing it.

    .venv/bin/python data.py        # writes data/train.npz, data/test.npz
"""

import io
import math
from multiprocessing import Pool
from pathlib import Path

import numpy as np
import pandas as pd
from PIL import Image
from skimage.morphology import dilation, disk, skeletonize

from prepare import prepare

MNIST = Path("data/mnist/mnist")
# Training copies of each MNIST digit: plain, and mouse-drawn.
PLAIN_COPIES = 1
MOUSE_COPIES = 3


def load_mnist(split):
    """(images [n, 28, 28] uint8, white ink on black; labels [n])."""
    df = pd.read_parquet(MNIST / f"{split}-00000-of-00001.parquet")
    images = np.stack([np.array(Image.open(io.BytesIO(b["bytes"])).convert("L")) for b in df["image"]])
    return images, df["label"].to_numpy()


def slant(img, rng, size):
    """`img` blown up to about `size` px, rotated, sheared and stretched."""
    angle = math.radians(rng.uniform(-15, 15))
    shear = rng.uniform(-0.3, 0.3)
    sx, sy = rng.uniform(0.8, 1.15), rng.uniform(0.85, 1.15)
    k = size / 28
    # Output pixel -> input pixel: undo scale, shear and rotation about the
    # centre.
    c, s = math.cos(angle), math.sin(angle)
    fwd = np.array([[k * sx * c, k * sx * (-s + shear * c)], [k * sy * s, k * sy * (c + shear * s)]])
    inv = np.linalg.inv(fwd)
    out = int(size * 1.6)
    cx_in, cx_out = 14, out / 2
    offset = np.array([cx_in, cx_in]) - inv @ np.array([cx_out, cx_out])
    coeffs = (inv[0, 0], inv[0, 1], offset[0], inv[1, 0], inv[1, 1], offset[1])
    return np.array(Image.fromarray(img).transform((out, out), Image.AFFINE, coeffs, Image.BILINEAR))


def mouse(img, rng):
    """`img` redrawn the way someone would draw it in Paint."""
    size = rng.uniform(40, 220)
    big = slant(img, rng, size) >= 128
    skeleton = skeletonize(big)
    # Paint's brush is 4 px; people also draw with big brushes elsewhere.
    radius = int(rng.choice([1, 2, 2, 2, 3]))
    return dilation(skeleton, disk(radius)).astype(np.float32)


def plain(img, rng):
    """MNIST as it is, slightly slanted now and then."""
    if rng.random() < 0.5:
        return img.astype(np.float32) / 255
    return slant(img, rng, 28 * rng.uniform(1, 2)).astype(np.float32) / 255


def make(job):
    images, seed, copies_plain, copies_mouse = job
    rng = np.random.default_rng(seed)
    out = []
    for img in images:
        out += [prepare(plain(img, rng)) for _ in range(copies_plain)]
        out += [prepare(mouse(img, rng)) for _ in range(copies_mouse)]
    return out


def build(images, labels, copies_plain, copies_mouse, seed):
    chunks = [(images[i:i + 500], seed * 1000 + i // 500, copies_plain, copies_mouse) for i in range(0, len(images), 500)]
    with Pool() as pool:
        prepared = [x for chunk in pool.map(make, chunks) for x in chunk]
    x = (np.stack(prepared) * 255 + 0.5).astype(np.uint8)
    y = np.repeat(labels, copies_plain + copies_mouse)
    return x, y


def main():
    train, train_labels = load_mnist("train")
    test, test_labels = load_mnist("test")
    x, y = build(train, train_labels, PLAIN_COPIES, MOUSE_COPIES, seed=1)
    np.savez_compressed("data/train.npz", x=x, y=y)
    print(f"train: {len(x):,} digits")
    # Two test sets, from digits never trained on: MNIST as it is, and
    # mouse-drawn.
    plain_x, _ = build(test, test_labels, 1, 0, seed=2)
    mouse_x, _ = build(test, test_labels, 0, 1, seed=3)
    np.savez_compressed("data/test.npz", plain=plain_x, mouse=mouse_x, y=test_labels)
    print(f"test: {len(plain_x):,} plain and {len(mouse_x):,} mouse-drawn digits")


if __name__ == "__main__":
    main()
