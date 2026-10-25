"""Shareable benchmark summaries; diagnostic details belong in local artifacts."""
import json
import re
from pathlib import PurePosixPath


def summarize(report):
    def compact(value):
        if isinstance(value, list):
            return [compact(item) for item in value]
        if not isinstance(value, dict):
            return value
        result = {}
        for key, item in value.items():
            if key in ("samples", "warnings"):
                result["sample_count" if key == "samples" else "warning_count"] = len(item)
            elif key == "log":
                continue
            elif key == "frame_samples_ms":
                # Historical native reports put these beside their statistics.
                continue
            elif key == "markers":
                paints = {}
                for marker in item:
                    match = re.fullmatch(r"reading: (.+ paint) ([\d.]+) ms", marker)
                    if match:
                        paints[match[1].replace(" ", "_")] = float(match[2])
                result["paint_ms"] = paints
                if "reading: last window closed" in item:
                    result["stop_reason"] = "window_closed"
            elif key == "results":
                result["failed"] = sum(not case["compiled"] for case in item)
                result["timeouts"] = sum(case["error"] == "Timed out" for case in item)
            elif key in ("file", "source") and isinstance(item, str):
                result[key] = PurePosixPath(item.replace("\\", "/")).name
            else:
                result[key] = compact(item)
        if "frame_samples_ms" in value:
            result["frame_wall_time_ms"]["sample_count"] = len(value["frame_samples_ms"])
        return result

    return compact(report) | {"schema": 2, "artifact": "summary"}


def validate_summary(data):
    """Fail closed on raw fields, unbounded arrays and machine-specific paths."""
    if len(data) > 16 * 1024:
        raise ValueError("summary exceeds 16 KiB; keep detailed results locally")
    report = json.loads(data)
    if not isinstance(report, dict) or report.get("schema") != 2 or report.get("artifact") != "summary":
        raise ValueError("expected a schema-2 summary")
    forbidden = {"samples", "frame_samples_ms", "markers", "results", "log", "stdout", "stderr", "warnings", "error", "errors"}

    def walk(value):
        if isinstance(value, dict):
            if forbidden.intersection(value):
                raise ValueError("raw samples, logs or diagnostics found in summary")
            for item in value.values():
                walk(item)
        elif isinstance(value, list):
            if len(value) > 64 or any(isinstance(item, (int, float)) for item in value):
                raise ValueError("sample-like or oversized array found in summary")
            for item in value:
                walk(item)
        elif isinstance(value, str):
            if value.startswith(("/", "\\\\")) or re.match(r"^[A-Za-z]:[\\/]", value):
                raise ValueError("absolute local path found in summary")
            if any(prefix in value for prefix in ("/Users/", "/home/", "/private/tmp/", "/var/folders/")):
                raise ValueError("machine-specific path found in summary")
    walk(report)
    return report
