import contextlib
import io
import os
import tempfile
import unittest
from datetime import date

from recur import RuleError, Schedule, dump_schedule, load_schedule, parse_rule
from recur.cli import main


def D(text):
    y, m, d = text.split("-")
    return date(int(y), int(m), int(d))


def ds(*texts):
    return [D(t) for t in texts]


def call(*argv):
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        code = main(list(argv))
    return code, out.getvalue(), err.getvalue()


SAMPLE = """# team schedule
rule: FREQ=WEEKLY;BYDAY=MO

START : 2026-01-01
EXRULE:FREQ=MONTHLY;BYDAY=1MO
add: 2026-01-20 , 2026-01-21
   EXCLUDE: 2026-01-19
"""

SAMPLE_DATES = ds("2026-01-12", "2026-01-20", "2026-01-21", "2026-01-26", "2026-02-09", "2026-02-16", "2026-02-23")


class ExclusionRuleTests(unittest.TestCase):
    def test_exclude_rule_removes_a_series(self):
        s = Schedule(D("2026-01-01"))
        s.add_rule("FREQ=WEEKLY;BYDAY=MO")
        s.exclude_rule("FREQ=MONTHLY;BYDAY=1MO")
        self.assertEqual(s.between(D("2026-01-01"), D("2026-02-28")),
                         ds("2026-01-12", "2026-01-19", "2026-01-26", "2026-02-09", "2026-02-16", "2026-02-23"))

    def test_exclusion_rule_keeps_its_own_count(self):
        s = Schedule(D("2026-01-01"))
        s.add_rule("FREQ=DAILY")
        s.exclude_rule(parse_rule("FREQ=WEEKLY;BYDAY=SA,SU;COUNT=2"))
        dates = s.between(D("2026-01-01"), D("2026-01-12"))
        self.assertEqual(dates, ds("2026-01-01", "2026-01-02", "2026-01-05", "2026-01-06", "2026-01-07",
                                   "2026-01-08", "2026-01-09", "2026-01-10", "2026-01-11", "2026-01-12"))

    def test_exclusion_rule_beats_extra_dates_and_next_after(self):
        s = Schedule(D("2026-01-01"))
        s.add_rule("FREQ=MONTHLY;BYMONTHDAY=1")
        s.add_date(D("2026-03-15"))
        s.exclude_rule("FREQ=WEEKLY;BYDAY=SU")
        self.assertEqual(s.between(D("2026-01-01"), D("2026-04-30")), ds("2026-01-01", "2026-04-01"))
        self.assertEqual(s.next_after(D("2026-01-01")), D("2026-04-01"))
        with self.assertRaises(RuleError):
            s.exclude_rule("FREQ=SOMETIMES")


class LoadScheduleTests(unittest.TestCase):
    def test_sample_file(self):
        s = load_schedule(SAMPLE)
        self.assertIsInstance(s, Schedule)
        self.assertEqual(s.between(D("2026-01-01"), D("2026-02-28")), SAMPLE_DATES)

    def test_lines_in_any_order_and_repeated(self):
        text = "RULE:FREQ=MONTHLY;BYMONTHDAY=10\nexclude:2026-02-10,2026-03-10\nRule:FREQ=MONTHLY;BYMONTHDAY=20\n" \
               "ADD:2025-12-31\nADD:2026-01-05\nstart:2026-01-01\n"
        s = load_schedule(text)
        self.assertEqual(s.between(D("2025-12-01"), D("2026-03-31")),
                         ds("2025-12-31", "2026-01-05", "2026-01-10", "2026-01-20", "2026-02-20", "2026-03-20"))

    def test_structural_errors_name_the_line(self):
        cases = [("START:2026-01-01\nRULE FREQ=DAILY\n", 2),
                 ("# comment\n\nSTART:2026-01-01\nCOLOR:red\n", 4),
                 ("START:2026-01-01\n\n\nSTART:2026-01-02\n", 4)]
        for text, n in cases:
            with self.subTest(text=text):
                with self.assertRaises(RuleError) as ctx:
                    load_schedule(text)
                self.assertTrue(str(ctx.exception).startswith(f"line {n}: "), str(ctx.exception))

    def test_value_errors_name_the_line(self):
        cases = [("START:2026-01-01\n\nRULE:FREQ=DAILY;COUNT=0\n", 3),
                 ("START:2026-02-30\n", 1),
                 ("START:2026-01-01\nADD:2026-01-05, 2026-13-01\n", 2),
                 ("EXRULE:FREQ=SOMETIMES\nSTART:2026-01-01\n", 1),
                 ("START:2026-01-01\n# x\nEXCLUDE:5 Jan\n", 3)]
        for text, n in cases:
            with self.subTest(text=text):
                with self.assertRaises(RuleError) as ctx:
                    load_schedule(text)
                self.assertTrue(str(ctx.exception).startswith(f"line {n}: "), str(ctx.exception))

    def test_missing_start(self):
        for text in ["", "# only a comment\n", "RULE:FREQ=DAILY\nADD:2026-01-01\n"]:
            with self.subTest(text=text):
                with self.assertRaises(RuleError):
                    load_schedule(text)


class DumpScheduleTests(unittest.TestCase):
    def test_canonical_text(self):
        s = Schedule(D("2026-01-01"))
        s.add_rule("byday=fr;freq=weekly")
        s.add_rule("FREQ=DAILY;INTERVAL=1;COUNT=3")
        s.exclude_rule("FREQ=MONTHLY;BYMONTHDAY=1")
        s.add_date(D("2026-02-01"))
        s.add_date(D("2025-12-25"))
        s.add_date(D("2026-02-01"))
        s.exclude(D("2026-01-09"))
        self.assertEqual(dump_schedule(s),
                         "START:2026-01-01\nRULE:FREQ=WEEKLY;BYDAY=FR\nRULE:FREQ=DAILY;COUNT=3\n"
                         "EXRULE:FREQ=MONTHLY;BYMONTHDAY=1\nADD:2025-12-25\nADD:2026-02-01\nEXCLUDE:2026-01-09\n")
        self.assertEqual(dump_schedule(Schedule(D("2026-05-06"))), "START:2026-05-06\n")

    def test_round_trip(self):
        first = dump_schedule(load_schedule(SAMPLE))
        self.assertEqual(first, "START:2026-01-01\nRULE:FREQ=WEEKLY;BYDAY=MO\nEXRULE:FREQ=MONTHLY;BYDAY=1MO\n"
                                "ADD:2026-01-20\nADD:2026-01-21\nEXCLUDE:2026-01-19\n")
        again = load_schedule(first)
        self.assertEqual(again.between(D("2026-01-01"), D("2026-02-28")), SAMPLE_DATES)
        self.assertEqual(dump_schedule(again), first)


class FileCliTests(unittest.TestCase):
    def test_schedule_command_takes_exrule(self):
        code, out, err = call("schedule", "--start", "2026-01-01", "--from", "2026-01-01", "--to", "2026-01-31",
                              "--rule", "FREQ=WEEKLY;BYDAY=TU,TH", "--exrule", "FREQ=WEEKLY;BYDAY=TH",
                              "--exrule", "FREQ=MONTHLY;BYDAY=-1TU")
        self.assertEqual((code, out, err), (0, "2026-01-06\n2026-01-13\n2026-01-20\n", ""))

    def test_file_and_format_commands(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "team.sched")
            with open(path, "w", encoding="utf-8") as f:
                f.write(SAMPLE)
            code, out, err = call("file", path, "--from", "2026-01-15", "--to", "2026-01-31")
            self.assertEqual((code, out, err), (0, "2026-01-20\n2026-01-21\n2026-01-26\n", ""))
            code, out, err = call("format", path)
            self.assertEqual((code, err), (0, ""))
            self.assertEqual(out, dump_schedule(load_schedule(SAMPLE)))

    def test_file_errors(self):
        with tempfile.TemporaryDirectory() as tmp:
            missing = os.path.join(tmp, "missing.sched")
            for argv in [("file", missing, "--from", "2026-01-01", "--to", "2026-01-31"), ("format", missing)]:
                with self.subTest(argv=argv):
                    code, out, err = call(*argv)
                    self.assertEqual((code, out), (2, ""))
                    self.assertTrue(err.startswith("error: "))
                    self.assertEqual(len(err.strip().splitlines()), 1)
            bad = os.path.join(tmp, "bad.sched")
            with open(bad, "w", encoding="utf-8") as f:
                f.write("START:2026-01-01\nRULE:FREQ=DAILY;COUNT=x\n")
            code, out, err = call("format", bad)
            self.assertEqual((code, out), (2, ""))
            self.assertTrue(err.startswith("error: line 2: "), err)
            code, _, err = call("file", bad, "--from", "2026-01-01")
            self.assertEqual(code, 2)
            self.assertNotEqual(err, "")
