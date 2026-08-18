#!/usr/bin/env python3
"""Fit hand-drawn app icon artwork onto the macOS app icon grid.

Artwork exports arrive as a bare rounded plate on transparency, sized however
the tool that made them felt like. macOS expects something specific: sampling
Mail / Notes / Safari on macOS 26 shows a plate of 814x814 centred in a 1024
canvas, sitting on a soft black shadow that peaks around 13% alpha and fades
over ~40px, pushed a few pixels down.

This script measures the plate in the source, scales it to 814x814, centres it,
and lays that shadow underneath.

The artwork itself is not in the repository -- it is a multi-megabyte export
kept under assets/raw, which .git/info/exclude leaves untracked -- so SOURCE has
to be named explicitly and assets/icon.png is the master everyone else builds
from.

Run: python3 make_appicon.py SOURCE [DEST]
  DEST defaults to ../../assets/icon.png
Then regenerate the bundle icons: pnpm tauri icon assets/icon.png
"""
import math
import os
import struct
import sys
import zlib

CANVAS = 1024
PLATE = 814  # opaque plate, as measured on Apple's own icons
INSET = (CANVAS - PLATE) // 2  # 105
MARGIN = 8  # source pixels kept around the plate so its soft edge survives

SHADOW_ALPHA = 0.26
SHADOW_BLUR = 15  # box blur radius
SHADOW_PASSES = 3  # repeated box blurs approximate a gaussian
SHADOW_DY = 6


def read_png(path):
    data = open(path, "rb").read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError(f"{path} is not a PNG")
    pos, idat, ihdr = 8, b"", None
    while pos < len(data):
        (length,) = struct.unpack(">I", data[pos : pos + 4])
        tag = data[pos + 4 : pos + 8]
        body = data[pos + 8 : pos + 8 + length]
        pos += 12 + length
        if tag == b"IHDR":
            ihdr = struct.unpack(">IIBBBBB", body)
        elif tag == b"IDAT":
            idat += body
        elif tag == b"IEND":
            break
    w, h, depth, colour, compression, filter_method, interlace = ihdr
    if (depth, colour) != (8, 6):
        raise ValueError(f"{path} must be 8-bit RGBA, got depth={depth} colour={colour}")
    if (compression, filter_method, interlace) != (0, 0, 0):
        raise ValueError(f"{path} must be a plain deflate, non-interlaced PNG")
    raw = zlib.decompress(idat)
    stride = w * 4
    out = bytearray(h * stride)
    prev = bytearray(stride)
    pos = 0
    for y in range(h):
        filt = raw[pos]
        pos += 1
        line = bytearray(raw[pos : pos + stride])
        pos += stride
        if filt == 1:
            for i in range(4, stride):
                line[i] = (line[i] + line[i - 4]) & 255
        elif filt == 2:
            for i in range(stride):
                line[i] = (line[i] + prev[i]) & 255
        elif filt == 3:
            for i in range(stride):
                left = line[i - 4] if i >= 4 else 0
                line[i] = (line[i] + ((left + prev[i]) >> 1)) & 255
        elif filt == 4:
            for i in range(stride):
                a = line[i - 4] if i >= 4 else 0
                b = prev[i]
                c = prev[i - 4] if i >= 4 else 0
                pa, pb, pc = abs(b - c), abs(a - c), abs(a + b - 2 * c)
                pred = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                line[i] = (line[i] + pred) & 255
        elif filt != 0:
            raise ValueError(f"unknown PNG filter {filt}")
        out[y * stride : (y + 1) * stride] = line
        prev = line
    return w, h, out


def plate_bounds(w, h, px):
    """Bounding box of the opaque plate, ignoring its antialiased rim."""
    xs = [x for x in range(w) if any(px[(y * w + x) * 4 + 3] >= 250 for y in range(h))]
    ys = [y for y in range(h) if any(px[(y * w + x) * 4 + 3] >= 250 for x in range(w))]
    if not xs or not ys:
        raise ValueError("found no opaque plate in the source")
    return xs[0], ys[0], xs[-1], ys[-1]


def resample(w, h, px, src, dst):
    """Area-average the source rect into the destination rect, premultiplied.

    src is (x0, y0, x1, y1) in source pixel edges, dst the same in destination
    pixels. Returns a flat list of premultiplied RGBA floats, canvas-sized.
    """
    sx0, sy0, sx1, sy1 = src
    dx0, dy0, dx1, dy1 = dst
    xstep = (sx1 - sx0) / (dx1 - dx0)
    ystep = (sy1 - sy0) / (dy1 - dy0)

    # Each destination pixel averages the source interval it covers. Intervals
    # are normalised by their full width, not by the samples found, so a source
    # rect reaching past the artwork reads as transparent there rather than
    # wrapping around into whatever pixels sit at that offset.
    def span(a, b, limit):
        return range(max(0, math.floor(a)), min(limit, math.ceil(b)))

    # Horizontal pass: one destination-width row per source row.
    cols = [
        (sx0 + (dx - dx0) * xstep, sx0 + (dx - dx0 + 1) * xstep) for dx in range(dx0, dx1)
    ]
    cols = [(a, b, span(a, b, w)) for a, b in cols]
    rows = {}
    for sy in span(sy0, sy1, h):
        acc = []
        base = sy * w * 4
        for a, b, xs in cols:
            r = g = bl = al = 0.0
            for sx in xs:
                cover = min(b, sx + 1) - max(a, sx)
                if cover <= 0:
                    continue
                i = base + sx * 4
                alpha = px[i + 3] / 255
                r += px[i] * alpha * cover
                g += px[i + 1] * alpha * cover
                bl += px[i + 2] * alpha * cover
                al += alpha * cover
            wt = b - a
            acc.append((r / wt, g / wt, bl / wt, al / wt))
        rows[sy] = acc

    # Vertical pass, straight into the canvas.
    canvas = [0.0] * (CANVAS * CANVAS * 4)
    for dy in range(max(0, dy0), min(CANVAS, dy1)):
        a = sy0 + (dy - dy0) * ystep
        b = a + ystep
        ys = [sy for sy in span(a, b, h) if sy in rows]
        wt = b - a
        for dx in range(dx1 - dx0):
            if not 0 <= dx0 + dx < CANVAS:
                continue
            r = g = bl = al = 0.0
            for sy in ys:
                cover = min(b, sy + 1) - max(a, sy)
                if cover <= 0:
                    continue
                sr, sg, sb, sa = rows[sy][dx]
                r += sr * cover
                g += sg * cover
                bl += sb * cover
                al += sa * cover
            i = (dy * CANVAS + dx0 + dx) * 4
            canvas[i] = r / wt
            canvas[i + 1] = g / wt
            canvas[i + 2] = bl / wt
            canvas[i + 3] = al / wt
    return canvas


def box_blur(mask, radius, passes):
    """Blur a CANVAS-sized alpha mask: prefix-sum box passes, each transposing.

    An even number of passes leaves the mask in its original orientation, so
    `passes` runs of a horizontal and a vertical blur approximate a gaussian.
    """
    for _ in range(2 * passes):
        out = [0.0] * (CANVAS * CANVAS)
        for y in range(CANVAS):
            row = mask[y * CANVAS : (y + 1) * CANVAS]
            acc = [0.0]
            for v in row:
                acc.append(acc[-1] + v)
            for x in range(CANVAS):
                lo = max(0, x - radius)
                hi = min(CANVAS, x + radius + 1)
                out[x * CANVAS + y] = (acc[hi] - acc[lo]) / (hi - lo)
        mask = out  # transposed; the next pass blurs the other axis
    return mask


def build(source):
    w, h, px = read_png(source)
    x0, y0, x1, y1 = plate_bounds(w, h, px)
    print(f"plate: {x1 - x0 + 1}x{y1 - y0 + 1} at ({x0},{y0}) in {w}x{h}")

    # Keep a margin of source pixels so the plate's soft rim is not clipped, and
    # let the destination rect grow by the same amount in destination units.
    mx = round(MARGIN * PLATE / (x1 - x0 + 1))
    my = round(MARGIN * PLATE / (y1 - y0 + 1))
    canvas = resample(
        w,
        h,
        px,
        (x0 - MARGIN, y0 - MARGIN, x1 + MARGIN + 1, y1 + MARGIN + 1),
        (INSET - mx, INSET - my, INSET + PLATE + mx, INSET + PLATE + my),
    )

    mask = [canvas[i * 4 + 3] for i in range(CANVAS * CANVAS)]
    if SHADOW_DY:
        shifted = [0.0] * (CANVAS * CANVAS)
        for y in range(SHADOW_DY, CANVAS):
            src = (y - SHADOW_DY) * CANVAS
            shifted[y * CANVAS : y * CANVAS + CANVAS] = mask[src : src + CANVAS]
        mask = shifted
    shadow = box_blur(mask, SHADOW_BLUR, SHADOW_PASSES)

    out = bytearray()
    for y in range(CANVAS):
        for x in range(CANVAS):
            i = (y * CANVAS + x) * 4
            r, g, b, a = canvas[i : i + 4]  # premultiplied
            sa = shadow[y * CANVAS + x] * SHADOW_ALPHA
            a = a + sa * (1 - a)  # the shadow sits under the artwork
            if a <= 0:
                out.extend((0, 0, 0, 0))
                continue
            out.extend(
                (
                    min(255, round(r / a)),
                    min(255, round(g / a)),
                    min(255, round(b / a)),
                    min(255, round(a * 255)),
                )
            )
    return out


def chunk(tag, data):
    return (
        struct.pack(">I", len(data))
        + tag
        + data
        + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    )


def scanline(line, prev, kind):
    """One PNG-filtered scanline. Filtering a photographic plate pays: picking
    the cheapest filter per row shrinks this icon by ~14% over no filtering."""
    out = bytearray(len(line))
    for i in range(len(line)):
        left = line[i - 4] if i >= 4 else 0
        up = prev[i]
        upleft = prev[i - 4] if i >= 4 else 0
        if kind == 0:
            out[i] = line[i]
        elif kind == 1:
            out[i] = (line[i] - left) & 255
        elif kind == 2:
            out[i] = (line[i] - up) & 255
        elif kind == 3:
            out[i] = (line[i] - ((left + up) >> 1)) & 255
        else:
            pa, pb, pc = abs(up - upleft), abs(left - upleft), abs(left + up - 2 * upleft)
            pred = left if (pa <= pb and pa <= pc) else (up if pb <= pc else upleft)
            out[i] = (line[i] - pred) & 255
    return out


def encode_png(width, height, rgba):
    stride = width * 4
    raw = bytearray()
    prev = bytearray(stride)
    for y in range(height):
        line = rgba[y * stride : (y + 1) * stride]
        best = None
        for kind in range(5):
            candidate = scanline(line, prev, kind)
            cost = sum(v if v < 128 else 256 - v for v in candidate)
            if best is None or cost < best[0]:
                best = (cost, kind, candidate)
        raw.append(best[1])
        raw.extend(best[2])
        prev = line
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
        + chunk(b"IEND", b"")
    )


def main():
    here = os.path.dirname(os.path.abspath(__file__))
    root = os.path.abspath(os.path.join(here, "..", ".."))
    if len(sys.argv) < 2:
        sys.exit("usage: make_appicon.py SOURCE [DEST]  (SOURCE is the artwork export)")
    source = sys.argv[1]
    dest = sys.argv[2] if len(sys.argv) > 2 else os.path.join(root, "assets/icon.png")
    png = encode_png(CANVAS, CANVAS, build(source))
    with open(dest, "wb") as f:
        f.write(png)
    print("wrote", dest, f"{CANVAS}x{CANVAS}", len(png), "bytes")


if __name__ == "__main__":
    main()
