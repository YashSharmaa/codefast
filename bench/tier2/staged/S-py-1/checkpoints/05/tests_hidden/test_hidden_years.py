import contextlib
import io
import unittest
from datetime import date

from recur import RecurError, Schedule, expand, parse_rule
from recur.cli import main


def D(text):
    y, m, d = text.split("-")
    return date(int(y), int(m), int(d))


def run(rule, start, **kw):
    return expand(parse_rule(rule), D(start), **kw)


def ds(*texts):
    return [D(t) for t in texts]


class ShortMonthTests(unittest.TestCase):
    def test_monthly_31st_uses_last_day(self):
        self.assertEqual(run("FREQ=MONTHLY;COUNT=4", "2026-01-31"),
                         ds("2026-01-31", "2026-02-28", "2026-03-31", "2026-04-30"))

    def test_monthly_30th_in_leap_february(self):
        self.assertEqual(run("FREQ=MONTHLY;COUNT=3", "2024-01-30"), ds("2024-01-30", "2024-02-29", "2024-03-30"))

    def test_yearly_leap_day_uses_february_28(self):
        self.assertEqual(run("FREQ=YEARLY;COUNT=5", "2024-02-29"),
                         ds("2024-02-29", "2025-02-28", "2026-02-28", "2027-02-28", "2028-02-29"))

    def test_yearly_bymonth_with_start_day_31(self):
        self.assertEqual(run("FREQ=YEARLY;BYMONTH=2,4;COUNT=4", "2026-01-31"),
                         ds("2026-02-28", "2026-04-30", "2027-02-28", "2027-04-30"))

    def test_interval_and_until_with_short_months(self):
        self.assertEqual(run("FREQ=MONTHLY;INTERVAL=3;UNTIL=2027-03-01", "2026-05-31"),
                         ds("2026-05-31", "2026-08-31", "2026-11-30", "2027-02-28"))
        s = Schedule(D("2026-01-31"))
        s.add_rule("FREQ=MONTHLY")
        self.assertEqual(s.next_after(D("2026-02-01")), D("2026-02-28"))

    def test_listed_month_days_are_still_skipped(self):
        self.assertEqual(run("FREQ=MONTHLY;BYMONTHDAY=31;COUNT=3", "2026-01-01"),
                         ds("2026-01-31", "2026-03-31", "2026-05-31"))
        self.assertEqual(run("FREQ=MONTHLY;BYMONTHDAY=30,31;COUNT=3", "2026-02-01"),
                         ds("2026-03-30", "2026-03-31", "2026-04-30"))

    def test_cli_expand_uses_last_day(self):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = main(["expand", "FREQ=MONTHLY;COUNT=3", "--start", "2026-03-31"])
        self.assertEqual((code, out.getvalue()), (0, "2026-03-31\n2026-04-30\n2026-05-31\n"))


class WholeYearTests(unittest.TestCase):
    def test_yearly_ordinal_weekday_counts_in_the_year(self):
        self.assertEqual(run("FREQ=YEARLY;BYDAY=-1FR;COUNT=2", "2026-01-01"), ds("2026-12-25", "2027-12-31"))
        self.assertEqual(run("FREQ=YEARLY;BYDAY=20MO;COUNT=1", "2026-01-01"), ds("2026-05-18"))
        self.assertEqual(run("FREQ=YEARLY;INTERVAL=2;BYDAY=1MO;COUNT=3", "2026-01-01"),
                         ds("2026-01-05", "2028-01-03", "2030-01-07"))

    def test_yearly_plain_weekday_is_every_week_of_the_year(self):
        self.assertEqual(run("FREQ=YEARLY;BYDAY=SU;COUNT=3", "2026-12-20"), ds("2026-12-20", "2026-12-27", "2027-01-03"))
        self.assertEqual(len(run("FREQ=YEARLY;BYDAY=TH;UNTIL=2026-12-31", "2026-01-01")), 53)

    def test_yearly_monthday_in_every_month(self):
        self.assertEqual(run("FREQ=YEARLY;BYMONTHDAY=1;COUNT=3", "2026-03-15"), ds("2026-04-01", "2026-05-01", "2026-06-01"))
        self.assertEqual(run("FREQ=YEARLY;BYMONTHDAY=31;COUNT=3", "2026-01-01"), ds("2026-01-31", "2026-03-31", "2026-05-31"))

    def test_yearly_both_lists_intersect(self):
        self.assertEqual(run("FREQ=YEARLY;BYMONTHDAY=13;BYDAY=FR;COUNT=3", "2026-01-01"),
                         ds("2026-02-13", "2026-03-13", "2026-11-13"))
        self.assertEqual(run("FREQ=YEARLY;BYDAY=1MO;BYMONTHDAY=5;COUNT=2", "2026-01-01"), ds("2026-01-05", "2032-01-05"))

    def test_ordinal_ranges(self):
        self.assertEqual(parse_rule("FREQ=YEARLY;BYDAY=53TH,-53MO").byday, ((0, -53), (3, 53)))
        self.assertEqual(run("FREQ=YEARLY;BYDAY=53TH;COUNT=2", "2026-01-01"), ds("2026-12-31", "2032-12-30"))
        for text in ["FREQ=YEARLY;BYDAY=54MO", "FREQ=YEARLY;BYDAY=-54FR", "FREQ=YEARLY;BYMONTH=1;BYDAY=6MO",
                     "FREQ=MONTHLY;BYDAY=20MO", "FREQ=MONTHLY;BYDAY=-6SU"]:
            with self.subTest(text=text):
                with self.assertRaises(RecurError):
                    parse_rule(text)
