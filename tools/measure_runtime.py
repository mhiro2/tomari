#!/usr/bin/env python3
"""Read bounded, process-only CPU and RSS samples on macOS 27 or later."""

import argparse
import json
import math
import os
import platform
import re
import subprocess
import sys
import time
from dataclasses import dataclass


class MeasurementError(Exception):
    def __init__(self, code, message):
        super().__init__(message)
        self.code = code


class ArgumentParser(argparse.ArgumentParser):
    def error(self, message):
        raise MeasurementError("invalid_arguments", "Invalid arguments; see --help.")


@dataclass(frozen=True)
class Snapshot:
    started: str
    cpu_seconds: float
    rss_bytes: int


def parse_cpu_time(value):
    match = re.fullmatch(r"(?:(\d+)-)?(?:(\d+):)?(\d+):(\d{2}(?:\.\d+)?)", value)
    if match is None:
        raise MeasurementError("invalid_sample", "Unexpected cumulative CPU time from ps.")
    days, hours, minutes, seconds = match.groups()
    if float(seconds) >= 60 or (hours is not None and int(minutes) >= 60):
        raise MeasurementError("invalid_sample", "Unexpected cumulative CPU time from ps.")
    if days is not None and (hours is None or int(hours) >= 24):
        raise MeasurementError("invalid_sample", "Unexpected cumulative CPU time from ps.")
    total = (
        int(days or 0) * 86400
        + int(hours or 0) * 3600
        + int(minutes) * 60
        + float(seconds)
    )
    if not math.isfinite(total):
        raise MeasurementError("invalid_sample", "Non-finite cumulative CPU time from ps.")
    return total


def read_snapshot(pid):
    try:
        result = subprocess.run(
            ["/bin/ps", "-p", str(pid), "-o", "lstart=", "-o", "time=", "-o", "rss=", "-o", "stat="],
            capture_output=True,
            text=True,
            timeout=2,
            check=False,
            env={**os.environ, "LC_ALL": "C", "TZ": "UTC"},
        )
    except subprocess.TimeoutExpired as error:
        raise MeasurementError("sample_timeout", "ps did not answer within two seconds.") from error
    except OSError as error:
        raise MeasurementError("sample_failed", "Could not run ps.") from error
    if result.returncode != 0 or not result.stdout.strip():
        raise MeasurementError("process_unavailable", "The selected process could not be read.")
    fields = result.stdout.split()
    if len(fields) != 8 or not fields[6].isascii() or not fields[6].isdigit():
        raise MeasurementError("invalid_sample", "Unexpected process statistics from ps.")
    if fields[7].startswith("Z"):
        raise MeasurementError("process_unavailable", "The selected process has exited.")
    return Snapshot(" ".join(fields[:5]), parse_cpu_time(fields[5]), int(fields[6]) * 1024)


def validate_options(pid, duration, interval):
    if pid <= 0 or pid > 2**31 - 1:
        raise MeasurementError("invalid_arguments", "PID must be a positive 32-bit integer.")
    if not math.isfinite(duration) or not 1 <= duration <= 28800:
        raise MeasurementError("invalid_arguments", "Duration must be between 1 and 28800 seconds.")
    if not math.isfinite(interval) or not 0.25 <= interval <= min(60, duration):
        raise MeasurementError(
            "invalid_arguments",
            "Interval must be between 0.25 and 60 seconds and no longer than duration.",
        )


def require_macos():
    version = platform.mac_ver()[0].split(".")[0]
    if sys.platform != "darwin" or not version.isdigit() or int(version) < 27:
        raise MeasurementError("unsupported_platform", "Measurement requires macOS 27 or later.")


def measure(pid, duration, interval, *, read=read_snapshot, clock=time.monotonic, sleep=time.sleep):
    report = {
        "format_version": 1,
        "status": "error",
        "pid": pid,
        "requested_duration_seconds": duration,
        "requested_interval_seconds": interval,
        "scope": "selected_process_only",
        "samples": [],
        "summary": None,
    }
    samples = report["samples"]
    try:
        first = read(pid)
        started = clock()
        current = first
        elapsed = 0.0
        while True:
            samples.append(
                {
                    "elapsed_seconds": round(elapsed, 6),
                    "cpu_seconds": current.cpu_seconds,
                    "rss_bytes": current.rss_bytes,
                }
            )
            if elapsed >= duration:
                break
            sleep(min(interval, duration - elapsed))
            previous = current
            current = read(pid)
            elapsed = clock() - started
            if current.started != first.started or current.cpu_seconds < previous.cpu_seconds:
                raise MeasurementError(
                    "process_changed", "The selected process identity or CPU counter changed."
                )
        report["summary"] = {
            "sample_count": len(samples),
            "elapsed_seconds": round(elapsed, 6),
            "cpu_percent": round((current.cpu_seconds - first.cpu_seconds) / elapsed * 100, 4),
            "rss_initial_bytes": first.rss_bytes,
            "rss_final_bytes": current.rss_bytes,
            "rss_peak_bytes": max(sample["rss_bytes"] for sample in samples),
            "rss_change_bytes": current.rss_bytes - first.rss_bytes,
        }
        report["status"] = "ok"
    except MeasurementError as error:
        report["error"] = {"code": error.code, "message": str(error)}
    except KeyboardInterrupt:
        report["error"] = {"code": "interrupted", "message": "Measurement was interrupted."}
    return report


def main(argv=None):
    parser = ArgumentParser(description=__doc__)
    parser.add_argument("--pid", required=True, type=int, help="PID selected in Activity Monitor")
    parser.add_argument("--duration", type=float, default=60, help="seconds, 1–28800 (default: 60)")
    parser.add_argument("--interval", type=float, default=1, help="seconds, 0.25–60 (default: 1)")
    try:
        args = parser.parse_args(argv)
        validate_options(args.pid, args.duration, args.interval)
        require_macos()
        report = measure(args.pid, args.duration, args.interval)
    except MeasurementError as error:
        report = {
            "format_version": 1,
            "status": "error",
            "error": {"code": error.code, "message": str(error)},
        }
    print(json.dumps(report, indent=2, allow_nan=False))
    return 0 if report["status"] == "ok" else 1


if __name__ == "__main__":
    sys.exit(main())
