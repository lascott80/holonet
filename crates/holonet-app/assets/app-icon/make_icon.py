"""Draws holonet.ico: the globe mark (assets/icons/globe.svg) in `accent`
on a `card` tile with a `line` border, at the sizes Windows asks for.

Standard library only. Run from anywhere: python make_icon.py
"""

import math
import os
import struct
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
SIZES = [16, 20, 24, 32, 40, 48, 64, 256]
ACCENT = (0x5C, 0xC8, 0xFF)
CARD = (0x1C, 0x1C, 0x1C)
LINE = (0x3D, 0x3D, 0x3D)
GLOBE = 0.84


def cubic(p0, p1, p2, p3, n=24):
    out = []
    for i in range(n + 1):
        t = i / n
        u = 1 - t
        out.append(tuple(u**3 * a + 3 * u * u * t * b + 3 * u * t * t * c + t**3 * d for a, b, c, d in zip(p0, p1, p2, p3)))
    return out


# The globe on the icon's 24-unit grid (globe.svg).
SEGMENTS = []
for poly in [[(3, 12), (21, 12)], cubic((12, 3), (15, 6), (15, 18), (12, 21)), cubic((12, 3), (9, 6), (9, 18), (12, 21))]:
    SEGMENTS += list(zip(poly, poly[1:]))


def seg_dist(px, py, a, b):
    (ax, ay), (bx, by) = a, b
    dx, dy = bx - ax, by - ay
    t = max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)))
    return math.hypot(px - ax - t * dx, py - ay - t * dy)


def rounded_rect_dist(x, y, lo, hi, r):
    # Signed distance to a rounded square [lo, hi]^2.
    c = (lo + hi) / 2
    h = (hi - lo) / 2 - r
    qx, qy = abs(x - c) - h, abs(y - c) - h
    return math.hypot(max(qx, 0), max(qy, 0)) + min(max(qx, qy), 0) - r


def render(size):
    ss = 4 if size < 64 else 2
    scale = size / 24.0
    # Small sizes get a thicker stroke so the globe still reads.
    stroke = 1.8 if size >= 48 else 2.2 if size >= 24 else 2.6
    half = stroke / 2
    tile_r = 5.0
    pixels = bytearray()
    for py in range(size):
        pixels.append(0)  # PNG filter: none
        for px in range(size):
            acc = [0.0, 0.0, 0.0, 0.0]
            for sy in range(ss):
                for sx in range(ss):
                    x = (px + (sx + 0.5) / ss) / scale
                    y = (py + (sy + 0.5) / ss) / scale
                    d_tile = rounded_rect_dist(x, y, 0.25, 23.75, tile_r)
                    if d_tile > 0:
                        continue
                    border = d_tile > -0.6 * (24 / size) * 1.5 and size >= 32
                    col = LINE if border else CARD
                    # The globe at GLOBE of the tile, leaving a margin.
                    gx, gy = 12 + (x - 12) / GLOBE, 12 + (y - 12) / GLOBE
                    d = abs(math.hypot(gx - 12, gy - 12) - 9)
                    for a, b in SEGMENTS:
                        d = min(d, seg_dist(gx, gy, a, b))
                    d *= GLOBE
                    if d <= half:
                        col = ACCENT
                    for i in range(3):
                        acc[i] += col[i]
                    acc[3] += 255
            n = ss * ss
            alpha = acc[3] / n
            if alpha == 0:
                pixels += b"\0\0\0\0"
            else:
                cover = acc[3] / 255
                pixels += bytes(round(acc[i] / cover) for i in range(3)) + bytes([round(alpha)])
    return png(size, bytes(pixels))


def png(size, raw):
    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def main():
    images = [(s, render(s)) for s in SIZES]
    header = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    entries, data = b"", b""
    for size, img in images:
        dim = 0 if size >= 256 else size
        entries += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(img), offset + len(data))
        data += img
    with open(os.path.join(HERE, "holonet.ico"), "wb") as f:
        f.write(header + entries + data)
    with open(os.path.join(HERE, "holonet-256.png"), "wb") as f:
        f.write(images[-1][1])
    print("wrote holonet.ico with", len(images), "sizes")


if __name__ == "__main__":
    main()
