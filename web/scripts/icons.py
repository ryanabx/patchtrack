"""Draw the app's icons: the list's filled circle on GitHub's header colour.

Standard library only; run from web/ to rewrite public/icons/*.png:

    python3 scripts/icons.py
"""
import struct
import zlib

BG = (0x24, 0x29, 0x2F)      # the app bar
RING = (0xF0, 0xF6, 0xFC)    # the bar's text
DOT = (0x2D, 0xA4, 0x4E)     # "applied" green


def png(path, size, radius_frac, corner_frac):
    """A circle (ring and dot) centred on a rounded square; 4x4 supersampled."""
    ss = 4
    rows = []
    for y in range(size):
        row = bytearray([0])  # filter: none
        for x in range(size):
            acc = [0, 0, 0, 0]
            for sy in range(ss):
                for sx in range(ss):
                    px, py = (x + (sx + .5) / ss) / size, (y + (sy + .5) / ss) / size
                    # rounded square (corner_frac 0: full bleed, for maskable)
                    c = corner_frac
                    dx, dy = max(c - px, 0, px - (1 - c)), max(c - py, 0, py - (1 - c))
                    if c and dx * dx + dy * dy > c * c:
                        continue
                    d = ((px - .5) ** 2 + (py - .5) ** 2) ** .5
                    colour = BG
                    if d < radius_frac * .62:
                        colour = DOT
                    elif d < radius_frac * .78:
                        colour = BG
                    elif d < radius_frac:
                        colour = RING
                    for i in range(3):
                        acc[i] += colour[i]
                    acc[3] += 255
            n = ss * ss
            row += bytes([acc[0] // n, acc[1] // n, acc[2] // n, acc[3] // n])
        rows.append(bytes(row))
    raw = zlib.compress(b"".join(rows), 9)

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
                + chunk(b"IDAT", raw) + chunk(b"IEND", b""))


png("public/icons/icon-192.png", 192, .36, .18)
png("public/icons/icon-512.png", 512, .36, .18)
# Maskable: full bleed, the circle inside the 80% safe zone.
png("public/icons/maskable-512.png", 512, .30, 0)
