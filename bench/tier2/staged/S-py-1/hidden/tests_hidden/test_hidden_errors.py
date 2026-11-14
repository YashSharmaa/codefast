import contextlib
import io
import os
import subprocess
import sys
import unittest
from datetime import date

import recur
import recur.errors
from recur import RecurError, Schedule, expand, load_schedule, parse_rule
from recur.cli import main
from recur.dates import parse_date

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def rule_error(text):
    try:
        parse_rule(text)
    except RecurError as exc:
        return exc
    raise AssertionError(f"no RecurError for {text!r}")


def file_error(text):
    try:
        load_schedule(text)
    except RecurError as exc:
        return exc
    raise AssertionError(f"no RecurError for {text!r}")


class RenameTests(unittest.TestCase):
    def test_recur_error_replaces_rule_error(self):
        self.assertTrue(issubclass(RecurError, ValueError))
        self.assertIs(recur.RecurError, recur.errors.RecurError)
        self.assertFalse(hasattr(recur, "RuleError"))
        self.assertFalse(hasattr(recur.errors, "RuleError"))

    def test_constructor_and_attributes(self):
        e = RecurError("boom")
        self.assertEqual((str(e), e.key, e.line), ("boom", None, None))
        e = RecurError("bad count", key="COUNT", line=3)
        self.assertEqual((str(e), e.key, e.line), ("bad count", "COUNT", 3))
        e = RecurError("x", "UNTIL", 7)
        self.assertEqual((e.key, e.line), ("UNTIL", 7))


class KeyTests(unittest.TestCase):
    def check(self, cases):
        for text, key in cases:
            with self.subTest(text=text):
                e = rule_error(text)
                self.assertEqual(e.key, key)
                self.assertIsNone(e.line)

    def test_value_errors_carry_their_key(self):
        self.check([("FREQ=DAILY;COUNT=0", "COUNT"), ("FREQ=DAILY;INTERVAL=x", "INTERVAL"),
                    ("FREQ=MONTHLY;BYMONTH=13", "BYMONTH"), ("FREQ=MONTHLY;BYMONTHDAY=-32", "BYMONTHDAY"),
                    ("FREQ=MONTHLY;BYDAY=XX", "BYDAY"), ("FREQ=MONTHLY;BYDAY=6MO", "BYDAY"),
                    ("FREQ=DAILY;UNTIL=2026-02-30", "UNTIL"), ("FREQ=DAILY;UNTIL=2026-2-1", "UNTIL"),
                    ("FREQ=MONTHLY;BYMONTH=1,", "BYMONTH"), ("FREQ=DAILY;COUNT=", "COUNT"),
                    ("freq=yearly;byday=54mo", "BYDAY")])

    def test_structural_errors(self):
        self.check([("color=red;freq=daily", "COLOR"), ("FREQ=DAILY;Count=2;COUNT=3", "COUNT"),
                    ("COUNT=3", "FREQ"), ("FREQ=HOURLY", "FREQ"), ("", None), ("   ", None),
                    ("FREQ=DAILY;", None), ("FREQ", None), ("FREQ=DAILY;;COUNT=2", None)])

    def test_combination_errors(self):
        self.check([("FREQ=DAILY;COUNT=2;UNTIL=2026-01-05", "UNTIL"), ("FREQ=WEEKLY;BYMONTHDAY=3", "BYMONTHDAY"),
                    ("FREQ=DAILY;BYDAY=1MO", "BYDAY"), ("FREQ=WEEKLY;BYDAY=-1FR", "BYDAY"),
                    ("FREQ=YEARLY;BYMONTH=2;BYDAY=6MO", "BYDAY")])

    def test_errors_outside_rules_have_no_key(self):
        errors = []
        for action in [lambda: parse_date("2026-13-01"),
                       lambda: expand(parse_rule("FREQ=DAILY"), date(2026, 1, 1)),
                       lambda: Schedule(date(2026, 1, 1)).between(date(2026, 2, 1), date(2026, 1, 1))]:
            with self.assertRaises(RecurError) as ctx:
                action()
            errors.append((ctx.exception.key, ctx.exception.line))
        self.assertEqual(errors, [(None, None)] * 3)

    def test_schedule_methods_keep_the_key(self):
        s = Schedule(date(2026, 1, 1))
        with self.assertRaises(RecurError) as ctx:
            s.add_rule("FREQ=DAILY;INTERVAL=0")
        self.assertEqual((ctx.exception.key, ctx.exception.line), ("INTERVAL", None))
        with self.assertRaises(RecurError) as ctx:
            s.exclude_rule("FREQ=NEVER")
        self.assertEqual((ctx.exception.key, ctx.exception.line), ("FREQ", None))


class LineTests(unittest.TestCase):
    def test_structural_file_errors_have_a_line(self):
        cases = [("START:2026-01-01\n# note\nRULE FREQ=DAILY\n", 3),
                 ("START:2026-01-01\nCOLOR:red\n", 2),
                 ("START:2026-01-01\n\n\nSTART:2026-01-01\n", 4)]
        for text, n in cases:
            with self.subTest(text=text):
                e = file_error(text)
                self.assertEqual((e.key, e.line), (None, n))
                self.assertTrue(str(e).startswith(f"line {n}: "))

    def test_rule_errors_in_a_file_keep_their_key(self):
        cases = [("START:2026-01-01\nRULE:FREQ=DAILY;COUNT=0\n", "COUNT", 2),
                 ("START:2026-01-01\n\n\n\nEXRULE:FREQ=WEEKLY;BYMONTHDAY=1\n", "BYMONTHDAY", 5),
                 ("RULE:FREQ=DAILY;UNTIL=2026-02-30\nSTART:2026-01-01\n", "UNTIL", 1),
                 ("START:2026-01-01\n#\nADD:2026-01-32\n", None, 3),
                 ("START:2026-02-30\n", None, 1),
                 ("START:2026-01-01\nEXCLUDE:2026-01-05,x\n", None, 2)]
        for text, key, n in cases:
            with self.subTest(text=text):
                e = file_error(text)
                self.assertEqual((e.key, e.line), (key, n))
                self.assertTrue(str(e).startswith(f"line {n}: "))

    def test_missing_start_has_no_line(self):
        e = file_error("RULE:FREQ=DAILY\n")
        self.assertEqual((e.key, e.line), (None, None))


class CliTests(unittest.TestCase):
    def test_cli_prints_the_message(self):
        message = str(rule_error("FREQ=DAILY;COUNT=0"))
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = main(["normalize", "FREQ=DAILY;COUNT=0"])
        self.assertEqual((code, out.getvalue(), err.getvalue()), (2, "", f"error: {message}\n"))
        p = subprocess.run([sys.executable, "-m", "recur", "describe", "FREQ=DAILY;BYDAY=1MO"],
                           cwd=ROOT, capture_output=True, text=True, timeout=30)
        self.assertEqual((p.returncode, p.stdout), (2, ""))
        self.assertEqual(p.stderr, f"error: {rule_error('FREQ=DAILY;BYDAY=1MO')}\n")
