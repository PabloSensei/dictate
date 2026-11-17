"""Draws the app icon (same design as docs/logo.svg) and writes assets/dictate.ico.

Each size is drawn on its own at 8x and scaled down, so small sizes stay crisp;
the ICO holds PNG images (Windows Vista and later).

Usage: python tools/make_icon.py
"""

import io
import struct
from pathlib import Path

from PIL import Image, ImageDraw

SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]
SS = 8  # supersampling


def draw(size: int) -> Image.Image:
    s = size * SS
    u = s / 128.0  # logo.svg is designed on a 128 grid
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    # vertical gradient #2a2a2f -> #141417 inside a rounded square
    grad = Image.new("RGBA", (s, s))
    top, bottom = (0x2A, 0x2A, 0x2F), (0x14, 0x14, 0x17)
    gd = ImageDraw.Draw(grad)
    for y in range(s):
        t = y / (s - 1)
        gd.line([(0, y), (s, y)], fill=tuple(round(a + (b - a) * t) for a, b in zip(top, bottom)) + (255,))
    mask = Image.new("L", (s, s), 0)
    inset = 4 * u if size >= 32 else 1 * u
    ImageDraw.Draw(mask).rounded_rectangle((inset, inset, s - inset, s - inset), radius=28 * u, fill=255)
    img.paste(grad, (0, 0), mask)
    d = ImageDraw.Draw(img)
    d.rounded_rectangle((inset, inset, s - inset, s - inset), radius=28 * u, outline=(255, 255, 255, 31), width=max(1, round(SS * 0.8)))
    # small sizes: fewer, thicker bars so they survive the downscale
    if size <= 24:
        dot = (36, 64, 12)
        bars = [(58, 52, 76), (76, 38, 90), (94, 50, 78)]
        bw = 11
    else:
        dot = (34, 64, 9)
        bars = [(53, 55, 73), (65, 44, 84), (77, 36, 92), (89, 47, 81), (101, 57, 71)]
        bw = 6
    cx, cy, r = dot
    d.ellipse(((cx - r) * u, (cy - r) * u, (cx + r) * u, (cy + r) * u), fill=(0xFF, 0x45, 0x3A, 255))
    for x, y0, y1 in bars:
        d.rounded_rectangle((x * u, y0 * u, (x + bw) * u, y1 * u), radius=bw / 2 * u, fill=(255, 255, 255, 255))
    return img.resize((size, size), Image.Resampling.LANCZOS)


def main() -> None:
    root = Path(__file__).resolve().parent.parent
    pngs = []
    for size in SIZES:
        buf = io.BytesIO()
        draw(size).save(buf, format="PNG", optimize=True)
        pngs.append(buf.getvalue())
    # ICONDIR + ICONDIRENTRY[] + PNG blobs
    out = struct.pack("<HHH", 0, 1, len(SIZES))
    offset = 6 + 16 * len(SIZES)
    for size, png in zip(SIZES, pngs):
        dim = 0 if size >= 256 else size
        out += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(png), offset)
        offset += len(png)
    out += b"".join(pngs)
    dst = root / "assets" / "dictate.ico"
    dst.parent.mkdir(exist_ok=True)
    dst.write_bytes(out)
    draw(256).save(root / "assets" / "dictate-256.png")
    print(dst, len(out), "bytes")


if __name__ == "__main__":
    main()
