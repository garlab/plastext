#!/usr/bin/env python3
"""Check the Git index, so staged raw data cannot hide behind a clean worktree."""
import subprocess
from pathlib import Path
from benchmark_summary import validate_summary

root = Path(__file__).resolve().parent.parent
paths = subprocess.check_output(["git", "ls-files", "--cached", "-z"], cwd=root).decode().split("\0")
errors = []
checked = 0
for path in paths:
    if path.endswith((".bench-raw.json", ".bench-raw.log")) or path.startswith("target/benchmarks/"):
        errors.append(f"{path}: raw benchmark artifacts must stay local")
    if not path.startswith("docs/benchmarks/") or path in ("docs/benchmarks/README.md", "docs/benchmarks/.gitignore"):
        continue
    if not path.endswith(".summary.json"):
        errors.append(f"{path}: only *.summary.json belongs in docs/benchmarks")
        continue
    try:
        data = subprocess.check_output(["git", "show", ":" + path], cwd=root)
        validate_summary(data)
        checked += 1
    except (ValueError, subprocess.CalledProcessError) as error:
        errors.append(f"{path}: {error}")
if errors:
    raise SystemExit("\n".join(errors))
print(f"Checked {checked} benchmark summaries; no tracked raw artifacts.")
