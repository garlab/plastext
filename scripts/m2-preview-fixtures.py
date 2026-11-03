#!/usr/bin/env python3
"""Deterministic PDF/TeX stress inputs. Writes only to the chosen local directory."""
import argparse
import hashlib
import json
from pathlib import Path


def pdf(path, pages, stream, size=(612, 792), images=0):
    # Small explicit PDF writer: no system fonts, downloads or generation tools.
    offsets = [0]
    with path.open("wb") as out:
        out.write(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n")

        def obj(number, data):
            assert number == len(offsets)
            offsets.append(out.tell())
            out.write(f"{number} 0 obj\n".encode() + data + b"\nendobj\n")

        obj(1, b"<< /Type /Catalog /Pages 2 0 R >>")
        first_page = 5 + images
        kids = " ".join(f"{first_page + i} 0 R" for i in range(pages))
        obj(2, f"<< /Type /Pages /Count {pages} /Kids [{kids}] >>".encode())
        obj(3, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
        obj(4, f"<< /Length {len(stream)} >>\nstream\n".encode() + stream + b"\nendstream")
        for image in range(images):
            # 48 MiB decoded and on disk per image. Incremental rows bound the
            # generator's own memory; unique objects exercise native PDF caches.
            offsets.append(out.tell())
            out.write(f"{5 + image} 0 obj\n<< /Type /XObject /Subtype /Image /Width 4096 /Height 4096 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length {4096 * 4096 * 3} >>\nstream\n".encode())
            for y in range(4096):
                out.write(bytes(v for x in range(4096) for v in ((x + image * 31) % 256, y % 256, (x ^ y) % 256)))
            out.write(b"\nendstream\nendobj\n")
        for page in range(pages):
            image_ref = f" /XObject << /Image {5 + page % images} 0 R >>" if images else ""
            obj(first_page + page, f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {size[0]} {size[1]}] /Resources << /Font << /F1 3 0 R >>{image_ref} >> /Contents 4 0 R >>".encode())
        xref = out.tell()
        out.write(f"xref\n0 {len(offsets)}\n0000000000 65535 f \n".encode())
        for offset in offsets[1:]:
            out.write(f"{offset:010} 00000 n \n".encode())
        out.write(f"trailer\n<< /Size {len(offsets)} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    args = parser.parse_args()
    root = args.directory
    root.mkdir(parents=True, exist_ok=True)
    pdf(root / "many-pages.pdf", 10_000, b"BT /F1 18 Tf 50 700 Td (Ten thousand pages) Tj ET")
    vectors = b"0.2 0.4 0.7 RG 0.3 w\n" + b"".join(
        f"{i % 600} {i * 17 % 780} m {i * 7 % 600} {i * 31 % 780} l S\n".encode()
        for i in range(50_000)
    )
    pdf(root / "dense-vectors.pdf", 100, vectors)
    pdf(root / "large-page.pdf", 10, b"q 24 0 0 18 0 0 cm\n" + vectors + b"\nQ", (14400, 14400))
    pdf(root / "large-images.pdf", 12, b"q 600 0 0 780 6 6 cm /Image Do Q", images=3)
    tex = root / "tex-project"
    tex.mkdir(exist_ok=True)
    (tex / "main.tex").write_text(
        "\\documentclass{article}\n\\usepackage{amsmath}\n\\begin{document}\n"
        + "".join(f"\\input{{chapter-{i:03}}}\n" for i in range(100))
        + "\\end{document}\n"
    )
    for chapter in range(100):
        (tex / f"chapter-{chapter:03}.tex").write_text(
            f"\\section{{Chapter {chapter}}}\\label{{chapter:{chapter}}}\n"
            + "See chapter \\ref{chapter:99}. A bounded preview with equations and a table.\\par\n" * 40
            + "\\[\\sum_{n=1}^{100} \\frac{1}{n^2} < \\frac{\\pi^2}{6}\\]\n"
            + "\\begin{tabular}{lr}Item & Count\\\\First & 123\\\\Second & 456\\\\\\end{tabular}\\clearpage\n"
        )
    manifest = []
    for path in sorted(root.rglob("*")):
        if path.is_file():
            with path.open("rb") as source:
                digest = hashlib.file_digest(source, "sha256").hexdigest()
            manifest.append({"file": path.relative_to(root).as_posix(), "bytes": path.stat().st_size, "sha256": digest})
    print(json.dumps(manifest, indent=2))


if __name__ == "__main__":
    main()
