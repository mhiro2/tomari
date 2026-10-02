"""Exercise measurement boundaries without observing or changing the host."""

import contextlib
import io
import json
import subprocess
import unittest
from unittest.mock import patch

import measure_runtime as runtime


class FakeClock:
    def __init__(self):
        self.now = 0
        self.sleeps = []

    def clock(self):
        return self.now

    def sleep(self, seconds):
        self.sleeps.append(seconds)
        self.now += seconds


class MeasurementTests(unittest.TestCase):
    def test_cpu_time_formats_and_invalid_values(self):
        for value, expected in [
            ("0:01.25", 1.25),
            ("123:45.67", 7425.67),
            ("2:03:04.50", 7384.5),
            ("1-02:03:04", 93784),
        ]:
            with self.subTest(value=value):
                self.assertEqual(runtime.parse_cpu_time(value), expected)
        for value in ["-1:00", "NaN", "1:60", "1:60:00", "1-24:00:00", "1-02:03"]:
            with self.subTest(value=value), self.assertRaises(runtime.MeasurementError):
                runtime.parse_cpu_time(value)

    def test_reads_only_explicit_statistics_and_converts_rss(self):
        result = subprocess.CompletedProcess(
            [], 0, "Fri Oct  2 12:34:56 2026 2:03.45 1024 S\n", ""
        )
        with patch.object(runtime.subprocess, "run", return_value=result) as run:
            self.assertEqual(
                runtime.read_snapshot(123),
                runtime.Snapshot("Fri Oct 2 12:34:56 2026", 123.45, 1048576),
            )
        self.assertEqual(
            run.call_args.args[0],
            ["/bin/ps", "-p", "123", "-o", "lstart=", "-o", "time=", "-o", "rss=", "-o", "stat="],
        )
        self.assertEqual(run.call_args.kwargs["timeout"], 2)
        self.assertEqual(run.call_args.kwargs["env"]["LC_ALL"], "C")

    def test_failed_or_malformed_samples_do_not_expose_command_output(self):
        for result in [
            subprocess.CompletedProcess([], 1, "", "private diagnostic"),
            subprocess.CompletedProcess([], 0, "", ""),
            subprocess.CompletedProcess([], 0, "unexpected private output", ""),
            subprocess.CompletedProcess([], 0, "Fri Oct 2 12:34:56 2026 0:00.00 0 Z", ""),
        ]:
            with self.subTest(result=result), patch.object(runtime.subprocess, "run", return_value=result):
                with self.assertRaises(runtime.MeasurementError) as raised:
                    runtime.read_snapshot(123)
                self.assertNotIn("private", str(raised.exception))
        for error, code in [
            (subprocess.TimeoutExpired([], 2), "sample_timeout"),
            (OSError("private details"), "sample_failed"),
        ]:
            with patch.object(runtime.subprocess, "run", side_effect=error):
                with self.assertRaises(runtime.MeasurementError) as raised:
                    runtime.read_snapshot(123)
                self.assertEqual(raised.exception.code, code)
                self.assertNotIn("private", str(raised.exception))

    def test_finite_measurement_uses_cpu_deltas_and_actual_elapsed_time(self):
        timer = FakeClock()
        snapshots = iter(
            [
                runtime.Snapshot("same", 10, 100),
                runtime.Snapshot("same", 12, 300),
                runtime.Snapshot("same", 13, 200),
            ]
        )

        def read(pid):
            self.assertEqual(pid, 123)
            timer.now += 0.25
            return next(snapshots)

        result = runtime.measure(123, 2, 1, read=read, clock=timer.clock, sleep=timer.sleep)
        self.assertEqual(result["status"], "ok")
        self.assertEqual(timer.sleeps, [1, 0.75])
        self.assertEqual(
            result["summary"],
            {
                "sample_count": 3,
                "elapsed_seconds": 2.25,
                "cpu_percent": 133.3333,
                "rss_initial_bytes": 100,
                "rss_final_bytes": 200,
                "rss_peak_bytes": 300,
                "rss_change_bytes": 100,
            },
        )

    def test_restart_counter_reset_and_exit_leave_incomplete_results(self):
        for changed in [runtime.Snapshot("different", 1, 100), runtime.Snapshot("same", 0.5, 100)]:
            timer = FakeClock()
            snapshots = iter([runtime.Snapshot("same", 1, 100), changed])
            report = runtime.measure(
                123, 2, 1, read=lambda _: next(snapshots), clock=timer.clock, sleep=timer.sleep
            )
            self.assertEqual(report["error"]["code"], "process_changed")
            self.assertIsNone(report["summary"])
            self.assertEqual(len(report["samples"]), 1)
        for error, code in [
            (runtime.MeasurementError("process_unavailable", "Process exited."), "process_unavailable"),
            (KeyboardInterrupt(), "interrupted"),
        ]:
            timer = FakeClock()
            with patch.object(
                runtime, "read_snapshot", side_effect=[runtime.Snapshot("same", 1, 100), error]
            ) as read:
                report = runtime.measure(123, 2, 1, read=read, clock=timer.clock, sleep=timer.sleep)
            self.assertEqual(report["status"], "error")
            self.assertEqual(report["error"]["code"], code)
            self.assertEqual(len(report["samples"]), 1)

    def test_invalid_bounds_including_nan_and_infinity(self):
        for args in [
            (0, 60, 1), (2**31, 60, 1), (1, 0, 1), (1, 28801, 1),
            (1, float("nan"), 1), (1, float("inf"), 1), (1, 60, 0),
            (1, 60, float("nan")), (1, 60, float("inf")), (1, 1, 2),
        ]:
            with self.subTest(args=args), self.assertRaises(runtime.MeasurementError):
                runtime.validate_options(*args)

    def test_cli_errors_are_nonzero_json_without_sampling(self):
        for argv in [[], ["--pid", "abc"], ["--pid", "1", "--duration", "nan"]]:
            output = io.StringIO()
            with contextlib.redirect_stdout(output), patch.object(runtime, "measure") as measure:
                self.assertEqual(runtime.main(argv), 1)
            self.assertEqual(json.loads(output.getvalue())["error"]["code"], "invalid_arguments")
            measure.assert_not_called()
        output = io.StringIO()
        with contextlib.redirect_stdout(output), patch.object(runtime.sys, "platform", "linux"):
            self.assertEqual(runtime.main(["--pid", "1"]), 1)
        self.assertEqual(json.loads(output.getvalue())["error"]["code"], "unsupported_platform")

    def test_macos_minimum_and_cli_report_exit_status(self):
        with patch.object(runtime.sys, "platform", "darwin"):
            with patch.object(runtime.platform, "mac_ver", return_value=("26.6", (), "")):
                with self.assertRaises(runtime.MeasurementError):
                    runtime.require_macos()
            with patch.object(runtime.platform, "mac_ver", return_value=("27.0", (), "")):
                runtime.require_macos()
        for status, exit_code in [("ok", 0), ("error", 1)]:
            output = io.StringIO()
            report = {"status": status}
            with contextlib.redirect_stdout(output), patch.object(runtime, "require_macos"), \
                    patch.object(runtime, "measure", return_value=report) as measure:
                self.assertEqual(runtime.main(["--pid", "123", "--duration", "2"]), exit_code)
            self.assertEqual(json.loads(output.getvalue()), report)
            measure.assert_called_once_with(123, 2, 1)


if __name__ == "__main__":
    unittest.main()
