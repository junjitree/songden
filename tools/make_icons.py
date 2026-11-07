"""Draws the player's icon: a lion face on savanna gold."""
import math, struct, sys, zlib

BG, MANE, FACE, INK = (0xF2, 0xA0, 0x3D), (0xB8, 0x56, 0x1A), (0xFF, 0xD2, 0x7A), (0x4A, 0x24, 0x08)
SS = 4  # supersamples per axis
MANE_PUFFS = 16

def inside_triangle(x, y, a, b, c):
    def side(p, q):
        return (q[0] - p[0]) * (y - p[1]) - (q[1] - p[1]) * (x - p[0])
    s1, s2, s3 = side(a, b), side(b, c), side(c, a)
    return (s1 >= 0 and s2 >= 0 and s3 >= 0) or (s1 <= 0 and s2 <= 0 and s3 <= 0)

def disc(x, y, cx, cy, r):
    return (x - cx) ** 2 + (y - cy) ** 2 <= r * r

def colour(x, y):  # x, y in 0..1; drawn front to back
    if inside_triangle(x, y, (0.455, 0.56), (0.545, 0.56), (0.5, 0.615)):
        return INK
    if disc(x, y, 0.42, 0.48, 0.028) or disc(x, y, 0.58, 0.48, 0.028):
        return INK
    if disc(x, y, 0.5, 0.52, 0.22):
        return FACE
    for cx in (0.33, 0.67):
        if disc(x, y, cx, 0.33, 0.055):
            return FACE
        if disc(x, y, cx, 0.33, 0.085):
            return MANE
    if disc(x, y, 0.5, 0.52, 0.31):
        return MANE
    for i in range(MANE_PUFFS):
        a = 2 * math.pi * i / MANE_PUFFS
        if disc(x, y, 0.5 + 0.31 * math.cos(a), 0.52 + 0.31 * math.sin(a), 0.075):
            return MANE
    return BG

def png(size, path):
    rows = []
    for py in range(size):
        row = bytearray([0])
        for px in range(size):
            acc = [0, 0, 0]
            for sy in range(SS):
                for sx in range(SS):
                    c = colour((px + (sx + 0.5) / SS) / size, (py + (sy + 0.5) / SS) / size)
                    acc = [a + v for a, v in zip(acc, c)]
            row += bytes(round(a / SS ** 2) for a in acc)
        rows.append(bytes(row))
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    out = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 2, 0, 0, 0))
    out += chunk(b"IDAT", zlib.compress(b"".join(rows), 9)) + chunk(b"IEND", b"")
    open(path, "wb").write(out)

for size in (180, 192, 512):
    png(size, f"{sys.argv[1]}/icon-{size}.png")
