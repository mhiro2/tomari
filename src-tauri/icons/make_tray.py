#!/usr/bin/env python3
"""Generate the menu-bar tray mark for Tomari from a single geometric master.

The mark is four disjoint closed paths -- three bracket members around an open
square plus the square itself -- laid out on a 32x32 grid. Everything is solid
black on full transparency so `icon_as_template(true)` can tint it: no shadow,
no blur, no translucent decoration.

Run: python3 make_tray.py
  -> tray.svg        vector master
  -> tray.png        32x32 RGBA
  -> tray@2x.png     64x64 RGBA

Optional: python3 make_tray.py --preview 512 writes preview.png at that size.
"""
import math
import os
import struct
import sys
import zlib

# --- Geometry, in the 32-unit grid of the SVG viewBox ------------------------
#
# The mark spans 24 of the 32 units (75%) so it holds its weight next to other
# menu-bar icons, and the seams between members stay at ~2 units so they survive
# a 32px export.

L, R = 4.0, 28.0  # outer left / right edge of the mark
TOP, BOT = 4.5, 27.5  # outer top / bottom edge
T = 4.5  # member thickness

LX, RX = L + T, R - T  # inner edge of the left / right upright
TY, BY = TOP + T, BOT - T  # inner edge of the top / bottom rail

RO = 5.5  # outer radius of a member's elbow
RI = 2.5  # inner fillet of that elbow
RC = 1.25  # end cap, and the corners of the centre square

TOP_RAIL_END = 17.75  # free end of the top member's rail
TOP_LEG_END = 15.75  # free end of the top member's upright
LEFT_LEG_START = 17.5  # free end of the lower-left member's upright
LEFT_RAIL_END = 17.5  # free end of the lower-left member's rail
RIGHT_RAIL_START = 19.5  # free end of the lower-right member's rail
RIGHT_LEG_START = 14.0  # free end of the lower-right member's upright

SQ0, SQ1 = 12.0, 20.0  # centre square, concentric with the mark

# Seams: 1.75 units down the left flank, 2.0 units across the bottom.
assert LEFT_LEG_START - TOP_LEG_END == 1.75
assert RIGHT_RAIL_START - LEFT_RAIL_END == 2.0


def top_member():
    """Upper-left member: a rail running right, an upright running down."""
    return [
        ("M", L + RO, TOP),
        ("L", TOP_RAIL_END - RC, TOP),
        ("A", RC, 1, TOP_RAIL_END, TOP + RC),
        ("L", TOP_RAIL_END, TY - RC),
        ("A", RC, 1, TOP_RAIL_END - RC, TY),
        ("L", LX + RI, TY),
        ("A", RI, 0, LX, TY + RI),
        ("L", LX, TOP_LEG_END - RC),
        ("A", RC, 1, LX - RC, TOP_LEG_END),
        ("L", L + RC, TOP_LEG_END),
        ("A", RC, 1, L, TOP_LEG_END - RC),
        ("L", L, TOP + RO),
        ("A", RO, 1, L + RO, TOP),
    ]


def left_member():
    """Lower-left member: an upright running down, a rail running right."""
    return [
        ("M", L + RC, LEFT_LEG_START),
        ("L", LX - RC, LEFT_LEG_START),
        ("A", RC, 1, LX, LEFT_LEG_START + RC),
        ("L", LX, BY - RI),
        ("A", RI, 0, LX + RI, BY),
        ("L", LEFT_RAIL_END - RC, BY),
        ("A", RC, 1, LEFT_RAIL_END, BY + RC),
        ("L", LEFT_RAIL_END, BOT - RC),
        ("A", RC, 1, LEFT_RAIL_END - RC, BOT),
        ("L", L + RO, BOT),
        ("A", RO, 1, L, BOT - RO),
        ("L", L, LEFT_LEG_START + RC),
        ("A", RC, 1, L + RC, LEFT_LEG_START),
    ]


def right_member():
    """Lower-right member: an upright running down, a rail running left."""
    return [
        ("M", RX, RIGHT_LEG_START + RC),
        ("A", RC, 1, RX + RC, RIGHT_LEG_START),
        ("L", R - RC, RIGHT_LEG_START),
        ("A", RC, 1, R, RIGHT_LEG_START + RC),
        ("L", R, BOT - RO),
        ("A", RO, 1, R - RO, BOT),
        ("L", RIGHT_RAIL_START + RC, BOT),
        ("A", RC, 1, RIGHT_RAIL_START, BOT - RC),
        ("L", RIGHT_RAIL_START, BY + RC),
        ("A", RC, 1, RIGHT_RAIL_START + RC, BY),
        ("L", RX - RI, BY),
        ("A", RI, 0, RX, BY - RI),
    ]


def centre_square():
    return [
        ("M", SQ0 + RC, SQ0),
        ("L", SQ1 - RC, SQ0),
        ("A", RC, 1, SQ1, SQ0 + RC),
        ("L", SQ1, SQ1 - RC),
        ("A", RC, 1, SQ1 - RC, SQ1),
        ("L", SQ0 + RC, SQ1),
        ("A", RC, 1, SQ0, SQ1 - RC),
        ("L", SQ0, SQ0 + RC),
        ("A", RC, 1, SQ0 + RC, SQ0),
    ]


PATHS = [top_member(), left_member(), right_member(), centre_square()]


# --- Vector output ----------------------------------------------------------


def num(v):
    s = f"{v:.4f}".rstrip("0").rstrip(".")
    return s if s else "0"


def path_data(path):
    out = []
    for cmd in path:
        if cmd[0] == "M":
            out.append(f"M{num(cmd[1])} {num(cmd[2])}")
        elif cmd[0] == "L":
            out.append(f"L{num(cmd[1])} {num(cmd[2])}")
        else:
            _, r, sweep, x, y = cmd
            out.append(f"A{num(r)} {num(r)} 0 0 {sweep} {num(x)} {num(y)}")
    return " ".join(out) + " Z"


def svg():
    paths = "\n".join(f'  <path d="{path_data(p)}" />' for p in PATHS)
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32" '
        'viewBox="0 0 32 32" fill="#000000">\n' + paths + "\n</svg>\n"
    )


# --- Rasterisation ----------------------------------------------------------


def arc_centre(p1, p2, r, sweep):
    """Centre of the quarter arc joining two axis-aligned points."""
    for c in ((p1[0], p2[1]), (p2[0], p1[1])):
        if abs(math.dist(c, p1) - r) > 1e-9 or abs(math.dist(c, p2) - r) > 1e-9:
            continue
        a1 = math.atan2(p1[1] - c[1], p1[0] - c[0])
        a2 = math.atan2(p2[1] - c[1], p2[0] - c[0])
        d = (a2 - a1) % (2 * math.pi)
        if sweep == 0:
            d -= 2 * math.pi
        if abs(abs(d) - math.pi / 2) < 1e-6:
            return c, a1, d
    raise ValueError(f"no quarter arc for {p1} -> {p2} r={r} sweep={sweep}")


def flatten(path, steps=24):
    """Turn a path into a closed polygon, arcs subdivided into line segments."""
    pts = [(path[0][1], path[0][2])]
    for cmd in path[1:]:
        if cmd[0] == "L":
            pts.append((cmd[1], cmd[2]))
        else:
            _, r, sweep, x, y = cmd
            c, a1, d = arc_centre(pts[-1], (x, y), r, sweep)
            for i in range(1, steps + 1):
                a = a1 + d * i / steps
                pts.append((c[0] + r * math.cos(a), c[1] + r * math.sin(a)))
    return pts


def coverage(size, ss=16):
    """Per-pixel coverage of the mark: subsampled in y, exact in x."""
    scale = size / 32.0
    cov = [0.0] * (size * size)
    for path in PATHS:
        poly = [(x * scale, y * scale) for x, y in flatten(path)]
        n = len(poly)
        for sy in range(size * ss):
            y = (sy + 0.5) / ss
            xs = []
            for i in range(n):
                x1, y1 = poly[i]
                x2, y2 = poly[(i + 1) % n]
                if y1 == y2:
                    continue
                if (y1 <= y < y2) or (y2 <= y < y1):
                    xs.append(x1 + (y - y1) / (y2 - y1) * (x2 - x1))
            if not xs:
                continue
            xs.sort()
            row = (sy // ss) * size
            for k in range(0, len(xs) - 1, 2):
                a = max(xs[k], 0.0)
                b = min(xs[k + 1], float(size))
                if b <= a:
                    continue
                ia = min(int(a), size - 1)
                ib = min(int(math.nextafter(b, 0.0)), size - 1)
                if ia == ib:
                    cov[row + ia] += (b - a) / ss
                else:
                    cov[row + ia] += (ia + 1 - a) / ss
                    for px in range(ia + 1, ib):
                        cov[row + px] += 1.0 / ss
                    cov[row + ib] += (b - ib) / ss
    return cov


def chunk(tag, data):
    return (
        struct.pack(">I", len(data))
        + tag
        + data
        + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    )


def write_png(path, size, ss=16):
    cov = coverage(size, ss)
    rows = bytearray()
    for y in range(size):
        rows.append(0)  # PNG filter type 0 for this scanline
        for x in range(size):
            a = min(1.0, max(0.0, cov[y * size + x]))
            rows.extend((0, 0, 0, round(a * 255)))
    png = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(bytes(rows), 9))
        + chunk(b"IEND", b"")
    )
    with open(path, "wb") as f:
        f.write(png)
    print("wrote", path, f"{size}x{size}", len(png), "bytes")


def main():
    here = os.path.dirname(os.path.abspath(__file__))
    if "--preview" in sys.argv:
        size = int(sys.argv[sys.argv.index("--preview") + 1])
        write_png("preview.png", size, ss=8)  # cwd, not the icons dir
        return
    svg_path = os.path.join(here, "tray.svg")
    with open(svg_path, "w") as f:
        f.write(svg())
    print("wrote", svg_path)
    write_png(os.path.join(here, "tray.png"), 32)
    write_png(os.path.join(here, "tray@2x.png"), 64)


if __name__ == "__main__":
    main()
