# Regenerate: python3 -I make_icon.py icon_1024.png, then sips/iconutil (see docs/parity/interface.md).
# Original Nox icon: night-sky squircle, a black island capsule hanging from
# the top, a crescent moon and a few stars. Pure Python rasterizer (no deps).
import math, struct, sys, zlib

N = 1024
SS = 3  # supersampling per axis

def squircle(x, y):
    # macOS-like superellipse, 824 px wide centred.
    cx, cy, r = 512, 512, 412
    dx, dy = abs(x - cx) / r, abs(y - cy) / r
    return dx ** 5 + dy ** 5 <= 1.0

def capsule(x, y):
    # Island: rounded rect 420x120 at top area.
    x0, x1, y0, y1, rad = 302, 722, 230, 350, 60
    cx = min(max(x, x0 + rad), x1 - rad)
    cy = min(max(y, y0 + rad), y1 - rad)
    return (x - cx) ** 2 + (y - cy) ** 2 <= rad * rad

def moon(x, y):
    a = (x - 512) ** 2 + (y - 610) ** 2 <= 150 ** 2
    b = (x - 575) ** 2 + (y - 560) ** 2 <= 130 ** 2
    return a and not b

STARS = [(330, 520, 9), (700, 690, 7), (640, 470, 6), (380, 730, 6), (760, 520, 5)]

def star(x, y):
    for sx, sy, r in STARS:
        d = abs(x - sx) ** 0.6 + abs(y - sy) ** 0.6
        if d <= r ** 0.6 * 1.6:
            return True
    return False

def sample(x, y):
    if not squircle(x, y):
        return (0, 0, 0, 0)
    t = y / N
    bg = (int(14 + 20 * t), int(18 + 26 * t), int(46 + 40 * t))
    # Soft glow below the island.
    g = math.exp(-(((x - 512) / 260) ** 2 + ((y - 300) / 120) ** 2))
    bg = tuple(min(255, int(c + g * k)) for c, k in zip(bg, (30, 60, 120)))
    if capsule(x, y):
        return (4, 4, 8, 255)
    if moon(x, y):
        return (250, 226, 150, 255)
    if star(x, y):
        return (235, 240, 255, 255)
    return bg + (255,)

rows = []
for py in range(N):
    row = bytearray([0])
    for px in range(N):
        acc = [0, 0, 0, 0]
        for sy in range(SS):
            for sx in range(SS):
                c = sample(px + (sx + 0.5) / SS, py + (sy + 0.5) / SS)
                a = c[3]
                acc[0] += c[0] * a; acc[1] += c[1] * a; acc[2] += c[2] * a; acc[3] += a
        n = SS * SS
        a = acc[3] / n
        if acc[3] > 0:
            row += bytes((int(acc[0] / acc[3]), int(acc[1] / acc[3]), int(acc[2] / acc[3]), int(a)))
        else:
            row += b"\0\0\0\0"
    rows.append(bytes(row))

def chunk(t, d):
    c = struct.pack(">I", len(d)) + t + d
    return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)

png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", N, N, 8, 6, 0, 0, 0))
png += chunk(b"IDAT", zlib.compress(b"".join(rows), 9)) + chunk(b"IEND", b"")
open(sys.argv[1], "wb").write(png)
