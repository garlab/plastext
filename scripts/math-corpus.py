#!/usr/bin/env python3
"""Compile every pinned KaTeX screenshot fixture in an isolated native process.

Compilation is necessary, not sufficient, for compatible visual output.
Keep failures and unsupported browser-context options in the report.
"""
import argparse
import hashlib
import json
import subprocess
import time
from pathlib import Path

root = Path(__file__).resolve().parent.parent
parser = argparse.ArgumentParser()
parser.add_argument("--binary", type=Path, default=root / "target/release/examples/spike")
parser.add_argument("--output", type=Path, default=Path("/tmp/plastext-math-corpus"))
parser.add_argument("--timeout", type=float, default=5)
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)
fixture = root / "crates/math/spike/katex/cases.json"
cases = json.loads(fixture.read_text())
results = []
for case in cases:
    name = case["name"]
    # JSON fixture names are used only as sanitized local artifact names.
    stem = "".join(c for c in name if c.isalnum() or c in "-_")
    source = args.output / (stem + ".tex")
    svg = args.output / (stem + ".svg")
    tex = case["tex"]
    for macro, value in case["options"].get("macros", {}).items():
        tex = "\\newcommand{" + macro + "}{" + value + "}" + tex
    source.write_text(tex)
    start = time.perf_counter()
    try:
        process = subprocess.run(
            [str(args.binary), "--case", str(source), str(svg),
             "display" if case["display"] else "inline"],
            capture_output=True, text=True, timeout=args.timeout,
        )
        compiled = process.returncode == 0 and svg.is_file()
        error = process.stderr[:16000] if not compiled else None
    except subprocess.TimeoutExpired:
        compiled, error = False, "Timed out"
    result = {"name": name, "compiled": compiled, "error": error,
              "process_ms": (time.perf_counter() - start) * 1000,
              "browser_context_options": case["options"]}
    results.append(result)
    print(("PASS " if compiled else "FAIL ") + name, flush=True)

passed = sum(r["compiled"] for r in results)
report = {"schema": 1, "upstream_commit": "8000fbd4e29cca2c0cb1e88b745af3ed2f436b66",
          "fixture_sha256": hashlib.sha256(fixture.read_bytes()).hexdigest(),
          "scope": "KaTeX screenshot fixtures: native compilation only, not visual parity; "
                   "HTML wrappers, CSS and noThrow/errorColor options are not implemented",
          "compiled": passed, "total": len(cases), "compile_percent": passed / len(cases) * 100,
          "visual_pass_percent": None, "results": results}
(args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
print(f"{passed}/{len(cases)} compiled; visual coverage unmeasured")
raise SystemExit(0 if passed / len(cases) >= .95 else 1)
