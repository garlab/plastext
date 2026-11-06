#!/usr/bin/env python3
"""Smoke-test the real welcome renderer with isolated settings and short-lived windows."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile


def main():
    repo = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", nargs="?", type=Path, default=repo / "target/debug/plastext-app")
    binary = parser.parse_args().binary.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="plastext-native-", dir="/tmp") as temp:
        root = Path(temp)
        folders = [root / f"Café… 中文 🦀 — long project name {i}" for i in range(10)]
        for folder in folders:
            folder.mkdir()
        for name, recent in [("empty", []), ("unicode-recents", folders)]:
            config = root / name / "plastext"
            config.mkdir(parents=True)
            (config / "recent-folders.json").write_text(
                json.dumps([str(path) for path in recent]), encoding="utf-8"
            )
            env = {key: value for key, value in os.environ.items()
                   if not key.startswith("PLASTEXT_PROFILE_")}
            env.update(
                XDG_CONFIG_HOME=str(config.parent),
                PLASTEXT_SOCKET=str(root / f"{name}.sock"),
                PLASTEXT_PROFILE_STARTUP="1",  # exits after drawing its first frame
                PLASTEXT_PROFILE_READING="1",  # keep the test window in the background
            )
            result = subprocess.run(
                [str(binary)], cwd=repo, env=env, capture_output=True, text=True, timeout=30
            )
            if result.returncode or "first frame drawn" not in result.stderr:
                raise SystemExit(
                    f"{name}: native startup failed ({result.returncode})\n"
                    f"{result.stdout}{result.stderr}"
                )
            print(f"{name}: native first frame passed")


if __name__ == "__main__":
    main()
