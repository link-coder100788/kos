#!/usr/bin/env python3
"""Draws KOS.app's icon: "KOS" in the system's own 8x8 pixel font on the
desktop's navy-to-purple gradient, with its taskbar along the bottom, as a
macOS rounded square.

    tools/make_icon.py KOS.icns       the app icon (uses sips and iconutil)
    tools/make_icon.py --png icon.png just the 1024x1024 picture

No image libraries: the picture is worked out pixel by pixel (edges
anti-aliased from their distance to the shape) and written as a PNG by
hand. tools/package_app.py calls make_icns.
"""

import argparse
import os
import re
import shutil
import subprocess
import struct
import sys
import tempfile
import zlib

SIZE = 1024
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FONT = os.path.join(ROOT, "userspace", "libc", "font8x8.c")

# The rounded square, on Apple's icon grid: 824 of the 1024 pixels.
BOX = (100, 100, 924, 924)
RADIUS = 185
# The desktop's colors (kernel/src/ui/wm.rs), a little brighter.
SKY_TOP = (0x22, 0x35, 0x5e)
SKY_BOTTOM = (0x58, 0x35, 0x70)
TASKBAR_TOP = (0x2a, 0x2e, 0x38)
TASKBAR_BOTTOM = (0x16, 0x18, 0x1e)
TASKBAR_LINE = (0x50, 0x56, 0x64)
START_TOP = (0x3c, 0x6c, 0xb0)
START_BOTTOM = (0x24, 0x48, 0x80)
LETTER_TOP = (0xd6, 0xd8, 0xff)
LETTER_BOTTOM = (0x92, 0x96, 0xd8)
LETTER_SHADOW = (0x0c, 0x10, 0x22)
TASKBAR_Y = 806
# One font pixel, in icon pixels.
DOT = 26


def font():
    """The 128 glyphs of userspace/libc/font8x8.c: 8 rows each, least
    significant bit leftmost."""
    with open(FONT) as f:
        rows = re.findall(r"\{((?:\s*0x[0-9A-Fa-f]{2},?){8})\s*\}", f.read())
    return [[int(b, 16) for b in re.findall(r"0x[0-9A-Fa-f]{2}", r)] for r in rows]


def text_dots(text):
    """The set of (column, row) font pixels that are on, for text."""
    glyphs = font()
    dots = set()
    for i, ch in enumerate(text):
        for row, bits in enumerate(glyphs[ord(ch)]):
            for col in range(8):
                if bits >> col & 1:
                    dots.add((i * 8 + col, row))
    return dots


def mix(a, b, t):
    return tuple(a[i] + (b[i] - a[i]) * t for i in range(3))


def clamp(x):
    return 0.0 if x < 0 else 1.0 if x > 1 else x


def box_distance(x, y, box, radius):
    """Signed distance from (x, y) to a rounded rectangle (negative inside)."""
    x0, y0, x1, y1 = box
    cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
    qx = abs(x - cx) - ((x1 - x0) / 2 - radius)
    qy = abs(y - cy) - ((y1 - y0) / 2 - radius)
    outside = (max(qx, 0) ** 2 + max(qy, 0) ** 2) ** 0.5
    return outside + min(max(qx, qy), 0) - radius


def over(top, alpha, bottom):
    return mix(bottom, top, alpha)


def render():
    """The icon as SIZE rows of RGBA bytes."""
    dots = text_dots("KOS")
    columns = max(c for c, _ in dots) + 1
    rows = max(r for _, r in dots) + 1
    text_x = (SIZE - columns * DOT) // 2
    text_y = (BOX[1] + TASKBAR_Y) // 2 - rows * DOT // 2 - 10
    shadow = DOT * 2 // 5
    start = (BOX[0] + 60, TASKBAR_Y + 34, BOX[0] + 200, TASKBAR_Y + 92)

    out = []
    for y in range(SIZE):
        row = bytearray()
        py = y + 0.5
        sky = mix(SKY_TOP, SKY_BOTTOM, clamp((py - BOX[1]) / (TASKBAR_Y - BOX[1])))
        bar = mix(TASKBAR_TOP, TASKBAR_BOTTOM, clamp((py - TASKBAR_Y) / (BOX[3] - TASKBAR_Y)))
        letter = mix(LETTER_TOP, LETTER_BOTTOM, clamp((py - text_y) / (rows * DOT)))
        button = mix(START_TOP, START_BOTTOM, clamp((py - start[1]) / (start[3] - start[1])))
        for x in range(SIZE):
            px = x + 0.5
            d = box_distance(px, py, BOX, RADIUS)
            if d > 40:
                row += b"\0\0\0\0"
                continue
            # The soft shadow macOS icons sit on.
            ds = box_distance(px, py - 12, BOX, RADIUS)
            shade = 0.32 * clamp(1 - ds / 28) if ds > 0 else 0.32
            coverage = clamp(0.5 - d)
            if coverage == 0:
                row += bytes((0, 0, 0, round(shade * 255)))
                continue

            if py < TASKBAR_Y:
                color = sky
                # A glow behind the letters.
                gx, gy = (px - SIZE / 2) / 420, (py - (text_y + rows * DOT / 2)) / 300
                color = over((0x8a, 0x7c, 0xc8), 0.22 * clamp(1 - (gx * gx + gy * gy)), color)
                col, r = int((px - text_x) // DOT), int((py - text_y) // DOT)
                scol, sr = int((px - text_x - shadow) // DOT), int((py - text_y - shadow) // DOT)
                if px >= text_x and py >= text_y and (col, r) in dots:
                    color = letter
                elif px >= text_x + shadow and py >= text_y + shadow and (scol, sr) in dots:
                    color = LETTER_SHADOW
            else:
                color = TASKBAR_LINE if py < TASKBAR_Y + 4 else bar
                if box_distance(px, py, start, 10) <= 0:
                    color = button

            # A faint light rim inside the edge.
            rim = clamp(1 - abs(d + 3) / 2.5) * 0.18
            color = over((255, 255, 255), rim, color)
            # Edge pixels: the icon over its shadow.
            alpha = coverage + shade * (1 - coverage)
            color = tuple(c * coverage / alpha for c in color)
            row += bytes((round(color[0]), round(color[1]), round(color[2]), round(alpha * 255)))
        out.append(bytes(row))
    return out


def write_png(path, rows):
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xffffffff)

    raw = b"".join(b"\0" + r for r in rows)
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", struct.pack(">IIBBBBB", len(rows[0]) // 4, len(rows), 8, 6, 0, 0, 0)))
        f.write(chunk(b"IDAT", zlib.compress(raw, 9)))
        f.write(chunk(b"IEND", b""))


def make_icns(out_path):
    """Draws the icon and writes it as an .icns with every size macOS uses."""
    with tempfile.TemporaryDirectory() as tmp:
        big = os.path.join(tmp, "icon.png")
        write_png(big, render())
        iconset = os.path.join(tmp, "KOS.iconset")
        os.mkdir(iconset)
        for size in (16, 32, 128, 256, 512):
            for scale in (1, 2):
                name = f"icon_{size}x{size}{'@2x' if scale == 2 else ''}.png"
                pixels = size * scale
                dest = os.path.join(iconset, name)
                if pixels == SIZE:
                    shutil.copy(big, dest)
                else:
                    subprocess.run(["sips", "-z", str(pixels), str(pixels), big, "--out", dest],
                                   check=True, capture_output=True)
        subprocess.run(["iconutil", "-c", "icns", iconset, "-o", out_path], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("out", nargs="?", help="the .icns to write")
    parser.add_argument("--png", help="write the 1024x1024 picture here instead")
    args = parser.parse_args()
    if args.png:
        write_png(args.png, render())
    elif args.out:
        make_icns(args.out)
    else:
        parser.error("give an .icns path, or --png")


if __name__ == "__main__":
    main()
