import contextlib
import io
import os
import subprocess
import sys
import unittest

from recur.cli import main

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def call(*argv):
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        code = main(list(argv))
    return code, out.getvalue(), err.getvalue()


class CliTests(unittest.TestCase):
    def test_normalize(self):
        self.assertEqual(call("normalize", "count=2;freq=weekly;interval=1;byday=we,mo"),
                         (0, "FREQ=WEEKLY;COUNT=2;BYDAY=MO,WE\n", ""))

    def test_expand_with_max_and_end(self):
        code, out, _ = call("expand", "FREQ=MONTHLY;BYDAY=-1FR", "--start", "2026-01-01", "--max", "2")
        self.assertEqual((code, out), (0, "2026-01-30\n2026-02-27\n"))
        code, out, _ = call("expand", "FREQ=DAILY;INTERVAL=10", "--start", "2026-01-01", "--end", "2026-01-21")
        self.assertEqual((code, out), (0, "2026-01-01\n2026-01-11\n2026-01-21\n"))

        self.assertEqual(call("next", "FREQ=WEEKLY;BYDAY=FR", "--start", "2026-01-01", "--after", "2026-01-09")[:2],
                         (0, "2026-01-16\n"))
        self.assertEqual(call("next", "FREQ=WEEKLY;COUNT=1", "--start", "2026-01-01", "--after", "2026-01-01")[:2],
                         (0, "none\n"))

    def test_schedule_with_repeated_options(self):
        code, out, _ = call("schedule", "--start", "2026-01-01", "--from", "2026-01-01", "--to", "2026-01-31",
                            "--rule", "FREQ=WEEKLY;BYDAY=FR", "--rule", "FREQ=MONTHLY;BYMONTHDAY=-1",
                            "--add", "2026-01-20", "--exclude", "2026-01-16")
        self.assertEqual(code, 0)
        self.assertEqual(out.split(), ["2026-01-02", "2026-01-09", "2026-01-20", "2026-01-23", "2026-01-30", "2026-01-31"])

    def test_rule_errors(self):
        code, out, err = call("normalize", "FREQ=DAILY;COUNT=0")
        self.assertEqual((code, out), (2, ""))
        self.assertTrue(err.startswith("error: "))
        self.assertEqual(len(err.strip().splitlines()), 1)
        code, out, err = call("expand", "FREQ=DAILY", "--start", "2026-01-01")
        self.assertEqual(code, 2)
        self.assertTrue(err.startswith("error: "))
        code, _, err = call("expand", "FREQ=DAILY;COUNT=1", "--start", "2026-13-01")
        self.assertEqual(code, 2)
        self.assertTrue(err.startswith("error: "))

    def test_usage_errors(self):
        for argv in [("frobnicate",), (), ("expand", "FREQ=DAILY;COUNT=1"), ("expand", "FREQ=DAILY", "--start", "2026-01-01", "--max", "x")]:
            with self.subTest(argv=argv):
                code, out, err = call(*argv)
                self.assertEqual(code, 2)
                self.assertNotEqual(err, "")

    def test_python_dash_m(self):
        p = subprocess.run([sys.executable, "-m", "recur", "expand", "FREQ=YEARLY;COUNT=2", "--start", "2024-02-29"],
                           cwd=ROOT, capture_output=True, text=True, timeout=30)
        self.assertEqual((p.returncode, p.stdout), (0, "2024-02-29\n2025-02-28\n"))
        p = subprocess.run([sys.executable, "-m", "recur", "normalize", "FREQ=SOMETIMES"],
                           cwd=ROOT, capture_output=True, text=True, timeout=30)
        self.assertEqual(p.returncode, 2)
        self.assertTrue(p.stderr.startswith("error: "))
