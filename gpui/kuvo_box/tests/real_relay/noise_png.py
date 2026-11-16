# A 2560x1640 RGB PNG of about `target` bytes: noise in some rows, flat in the rest (a screenshot's size).
import os, struct, sys, zlib
out, target = sys.argv[1], int(sys.argv[2])
w, h = 2560, 1640
raw_row = w * 3
noisy = min(h, max(1, target // raw_row))
rows = []
for y in range(h):
    rows.append(b"\x00" + (os.urandom(raw_row) if y < noisy else bytes([40, 44, 52]) * w))
data = zlib.compress(b"".join(rows), 6)
def chunk(kind, body):
    return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body) & 0xffffffff)
png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + chunk(b"IDAT", data) + chunk(b"IEND", b"")
open(out, "wb").write(png)
print(out, len(png))
