#!/usr/bin/env python3
"""Deterministic large M2 fixtures; generated files stay outside the repository."""
import hashlib
import json
import sys
from pathlib import Path

out = Path(sys.argv[1] if len(sys.argv) > 1 else "/tmp/plastext-m2")
out.mkdir(parents=True, exist_ok=True)
markdown = out / "large.md"
target = 100_000_000
paragraph = ("Readable **bold** and *italic* text, `inline code`, café, &amp; "
             "[section links](#large-document). " * 20)
section = ("\n## Repeated section\n\n" + paragraph +
           "\n\n- [x] Read this\n- [ ] Next item\n\n> [!NOTE]\n> A native preview.\n\n").encode()
with markdown.open("wb") as f:
    f.write(b"---\ntitle: Large document\n---\n# Large document\n\n")
    f.write(b"| Name | Value |\n| :--- | ---: |\n")
    for i in range(100_000):
        f.write(f"| row {i} | {i * 2} |\n".encode())
    f.write(b"\n```rust\n")
    f.write(b"let value = 42; // a bounded code item\n" * 100_000)
    f.write(b"```\n")
    while f.tell() + len(section) <= target:
        f.write(section)
    f.write(b"\n\n" + b"x" * (target - f.tell() - 2))

csv = out / "large.csv"
with csv.open("wb") as f:
    f.write(b"id,name,value\n")
    # Repeated 10,000-record batches avoid holding the 20M-row fixture in RAM.
    batch = b"".join(f'{i},"sample, quoted",{i * 2}\n'.encode() for i in range(10_000))
    for _ in range(2_000):
        f.write(batch)

manifest = []
for path in (markdown, csv):
    digest = hashlib.sha256()
    with path.open("rb") as f:
        while block := f.read(1024 * 1024):
            digest.update(block)
    manifest.append({"path": str(path), "bytes": path.stat().st_size, "sha256": digest.hexdigest()})
print(json.dumps(manifest, indent=2))
