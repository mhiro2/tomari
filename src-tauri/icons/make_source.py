#!/usr/bin/env python3
"""Generate a 1024x1024 RGBA source icon for Kanary using only the stdlib.

A charcoal squircle background with a minimalist canary-yellow bird mark.
Run: python3 make_source.py  ->  writes source.png next to this script.
"""
import os
import struct
import zlib

S = 1024


def inside_rounded(x, y, w, h, r):
    # Distance check against the four rounded corners.
    cx = min(max(x, r), w - r)
    cy = min(max(y, r), h - r)
    dx = x - cx
    dy = y - cy
    return dx * dx + dy * dy <= r * r


def in_circle(x, y, cx, cy, rad):
    return (x - cx) ** 2 + (y - cy) ** 2 <= rad * rad


def sign(ax, ay, bx, by, px, py):
    return (px - bx) * (ay - by) - (ax - bx) * (py - by)


def in_triangle(px, py, a, b, c):
    d1 = sign(a[0], a[1], b[0], b[1], px, py)
    d2 = sign(b[0], b[1], c[0], c[1], px, py)
    d3 = sign(c[0], c[1], a[0], a[1], px, py)
    has_neg = (d1 < 0) or (d2 < 0) or (d3 < 0)
    has_pos = (d1 > 0) or (d2 > 0) or (d3 > 0)
    return not (has_neg and has_pos)


def composite(base, top):
    ta = top[3] / 255.0
    return (
        round(top[0] * ta + base[0] * (1 - ta)),
        round(top[1] * ta + base[1] * (1 - ta)),
        round(top[2] * ta + base[2] * (1 - ta)),
        255,
    )


def build():
    radius = 225
    body = (512, 552)
    body_r = 250
    beak = [(740, 520), (740, 584), (838, 552)]
    eye = (576, 476)
    rows = bytearray()
    for y in range(S):
        rows.append(0)  # PNG filter type 0 for this scanline
        # Vertical background gradient.
        t = y / S
        bg = (
            round(44 * (1 - t) + 26 * t),
            round(46 * (1 - t) + 27 * t),
            round(58 * (1 - t) + 32 * t),
            255,
        )
        for x in range(S):
            if not inside_rounded(x, y, S, S, radius):
                rows.extend((0, 0, 0, 0))
                continue
            px = bg
            if in_circle(x, y, body[0], body[1], body_r):
                px = composite(px, (246, 201, 69, 255))
            if in_triangle(x, y, *beak):
                px = composite(px, (240, 150, 40, 255))
            if in_circle(x, y, eye[0], eye[1], 30):
                px = composite(px, (38, 30, 12, 255))
            rows.extend(px)
    return rows


def chunk(tag, data):
    return (
        struct.pack(">I", len(data))
        + tag
        + data
        + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    )


def main():
    raw = build()
    ihdr = struct.pack(">IIBBBBB", S, S, 8, 6, 0, 0, 0)
    png = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
        + chunk(b"IEND", b"")
    )
    out = os.path.join(os.path.dirname(os.path.abspath(__file__)), "source.png")
    with open(out, "wb") as f:
        f.write(png)
    print("wrote", out, len(png), "bytes")


if __name__ == "__main__":
    main()
