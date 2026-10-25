import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from benchmark_summary import summarize, validate_summary


class BenchmarkSummaryTests(unittest.TestCase):
    def test_native_failure_statistics_and_counts_survive(self):
        raw = {"file": "/Users/private/fixture.md", "log": "/private/tmp/session.log",
               "complete": False, "passed": False, "observed_frames": 3,
               "metrics": [{"samples": [1, 2, 30], "p50": 2, "p99": 30, "max": 30, "pass": False}]}
        result = summarize(raw)
        self.assertEqual(result["metrics"][0], {"sample_count": 3, "p50": 2, "p99": 30, "max": 30, "pass": False})
        self.assertFalse(result["complete"])
        self.assertEqual(result["file"], "fixture.md")
        validate_summary(json.dumps(result).encode())
        self.assertEqual(len(raw["metrics"][0]["samples"]), 3)

    def test_legacy_frames_and_math_details_stay_out(self):
        native = summarize({"runs": [{"frame_wall_time_ms": {"p99": 9}, "frame_samples_ms": [1, 9],
                                      "markers": ["reading: first paint 12.4 ms", "reading: last window closed"]}]})
        self.assertEqual(native["runs"][0]["frame_wall_time_ms"]["sample_count"], 2)
        self.assertEqual(native["runs"][0]["paint_ms"], {"first_paint": 12.4})
        self.assertEqual(native["runs"][0]["stop_reason"], "window_closed")
        math = summarize({"compiled": 0, "total": 1, "results": [{"compiled": False, "error": "Timed out"}]})
        self.assertEqual((math["failed"], math["timeouts"]), (1, 1))
        for report in (native, math):
            validate_summary(json.dumps(report).encode())

    def test_gate_rejects_raw_data_paths_and_growth(self):
        for bad in ({"samples": [1]}, {"nested": {"stderr": "private"}}, {"values": [1, 2]},
                    {"file": "/Users/private/a"}, {"source": "C:\\Users\\private\\a"},
                    {"cases": ["case"] * 65}, {"scope": "x" * 17000}):
            with self.assertRaises(ValueError):
                validate_summary(json.dumps({"schema": 2, "artifact": "summary"} | bad).encode())

    def test_gate_checks_staged_bytes_even_when_worktree_is_clean(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary)
            subprocess.run(["git", "init", "--quiet", str(repo)], check=True)
            scripts = repo / "scripts"
            scripts.mkdir()
            for name in ("check-benchmark-artifacts.py", "benchmark_summary.py"):
                shutil.copyfile(Path(__file__).parent / name, scripts / name)
            artifact = repo / "docs/benchmarks/run.summary.json"
            artifact.parent.mkdir(parents=True)
            raw = {"schema": 2, "artifact": "summary", "metrics": [{"samples": [1, 2]}]}
            artifact.write_text(json.dumps(raw))
            subprocess.run(["git", "add", "docs/benchmarks/run.summary.json"], cwd=repo, check=True)
            artifact.write_text(json.dumps(summarize(raw)))
            command = [sys.executable, str(scripts / "check-benchmark-artifacts.py")]
            result = subprocess.run(command, cwd=repo, capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("raw samples", result.stderr)
            subprocess.run(["git", "add", "docs/benchmarks/run.summary.json"], cwd=repo, check=True)
            result = subprocess.run(command, cwd=repo, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()
