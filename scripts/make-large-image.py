#!/usr/bin/env python3
"""Generate a huge grayscale PNG using bounded memory; outputs stay local."""
import argparse
from pathlib import Path
import struct
import zlib

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("output", type=Path)
parser.add_argument("--size", type=int, default=40_000)
args = parser.parse_args()
if not 1 <= args.size <= 40_000:
    parser.error("size must be between 1 and 40000")
args.output.parent.mkdir(parents=True, exist_ok=True)

with args.output.open("wb") as output:
    def chunk(kind, data):
        output.write(struct.pack(">I", len(data)))
        output.write(kind)
        output.write(data)
        output.write(struct.pack(">I", zlib.crc32(data, zlib.crc32(kind))))

    output.write(b"\x89PNG\r\n\x1a\n")
    chunk(b"IHDR", struct.pack(">IIBBBBB", args.size, args.size, 8, 0, 0, 0, 0))
    compressor = zlib.compressobj(level=1)
    pending = bytearray()
    for y in range(args.size):
        pending.extend(compressor.compress(b"\0" + bytes([y % 256]) * args.size))
        if len(pending) >= 256 * 1024:
            chunk(b"IDAT", pending)
            pending.clear()
    pending.extend(compressor.flush())
    if pending:
        chunk(b"IDAT", pending)
    chunk(b"IEND", b"")
print(f"{args.size} × {args.size}; {args.output.stat().st_size:,} encoded bytes")
