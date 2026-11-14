import itertools
import unittest
from datetime import date

from recur import RuleError, expand, occurrences, parse_rule


def D(text):
    y, m, d = text.split("-")
    return date(int(y), int(m), int(d))


def run(rule, start, **kw):
    return expand(parse_rule(rule), D(start), **kw)


def ds(*texts):
    return [D(t) for t in texts]


class DailyTests(unittest.TestCase):
    def test_interval(self):
        self.assertEqual(run("FREQ=DAILY;INTERVAL=3;COUNT=4", "2026-01-30"),
                         ds("2026-01-30", "2026-02-02", "2026-02-05", "2026-02-08"))

    def test_byday_filter_and_start_not_candidate(self):
        self.assertEqual(run("FREQ=DAILY;BYDAY=MO,WE;COUNT=4", "2026-01-01"),
                         ds("2026-01-05", "2026-01-07", "2026-01-12", "2026-01-14"))

        self.assertEqual(run("FREQ=DAILY;INTERVAL=2;BYDAY=MO;COUNT=3", "2026-01-01"),
                         ds("2026-01-05", "2026-01-19", "2026-02-02"))

    def test_negative_monthday_filter(self):
        self.assertEqual(run("FREQ=DAILY;BYMONTHDAY=-1;COUNT=3", "2026-01-15"),
                         ds("2026-01-31", "2026-02-28", "2026-03-31"))


class WeeklyTests(unittest.TestCase):
    def test_default_weekday_and_interval(self):
        self.assertEqual(run("FREQ=WEEKLY;INTERVAL=2;COUNT=3", "2026-01-01"),
                         ds("2026-01-01", "2026-01-15", "2026-01-29"))

    def test_byday_skips_days_before_start(self):
        self.assertEqual(run("FREQ=WEEKLY;BYDAY=MO,FR;COUNT=4", "2026-01-07"),
                         ds("2026-01-09", "2026-01-12", "2026-01-16", "2026-01-19"))

    def test_weeks_run_monday_to_sunday(self):
        self.assertEqual(run("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,SU;COUNT=4", "2026-01-10"),
                         ds("2026-01-11", "2026-01-19", "2026-01-25", "2026-02-02"))

        self.assertEqual(run("FREQ=WEEKLY;BYMONTH=2;COUNT=3", "2026-01-22"),
                         ds("2026-02-05", "2026-02-12", "2026-02-19"))


class MonthlyTests(unittest.TestCase):
    def test_31st_skips_short_months(self):
        self.assertEqual(run("FREQ=MONTHLY;COUNT=5", "2026-01-31"),
                         ds("2026-01-31", "2026-03-31", "2026-05-31", "2026-07-31", "2026-08-31"))

    def test_last_friday(self):
        self.assertEqual(run("FREQ=MONTHLY;BYDAY=-1FR;COUNT=4", "2026-01-01"),
                         ds("2026-01-30", "2026-02-27", "2026-03-27", "2026-04-24"))

        self.assertEqual(run("FREQ=MONTHLY;BYDAY=2TU,4TU;COUNT=4", "2026-01-14"),
                         ds("2026-01-27", "2026-02-10", "2026-02-24", "2026-03-10"))

    def test_first_and_last_day_every_other_month(self):
        self.assertEqual(run("FREQ=MONTHLY;INTERVAL=2;BYMONTHDAY=1,-1;COUNT=5", "2026-01-01"),
                         ds("2026-01-01", "2026-01-31", "2026-03-01", "2026-03-31", "2026-05-01"))

    def test_byday_and_bymonthday_intersect(self):
        self.assertEqual(run("FREQ=MONTHLY;BYDAY=FR;BYMONTHDAY=13;COUNT=3", "2026-01-01"),
                         ds("2026-02-13", "2026-03-13", "2026-11-13"))

    def test_missing_fifth_weekday_is_skipped(self):
        self.assertEqual(run("FREQ=MONTHLY;BYDAY=5MO;COUNT=3", "2026-01-01"),
                         ds("2026-03-30", "2026-06-29", "2026-08-31"))

    def test_bymonth_limits_months(self):
        self.assertEqual(run("FREQ=MONTHLY;BYMONTH=1,7;BYMONTHDAY=1;COUNT=3", "2026-01-15"),
                         ds("2026-07-01", "2027-01-01", "2027-07-01"))


class YearlyTests(unittest.TestCase):
    def test_leap_day_start(self):
        self.assertEqual(run("FREQ=YEARLY;COUNT=3", "2024-02-29"),
                         ds("2024-02-29", "2028-02-29", "2032-02-29"))

    def test_ordinal_weekday_within_listed_months(self):
        self.assertEqual(run("FREQ=YEARLY;BYMONTH=3,9;BYDAY=1SU;COUNT=3", "2026-01-01"),
                         ds("2026-03-01", "2026-09-06", "2027-03-07"))

    def test_interval_and_bymonth_uses_start_day(self):
        self.assertEqual(run("FREQ=YEARLY;INTERVAL=2;COUNT=3", "2026-05-10"),
                         ds("2026-05-10", "2028-05-10", "2030-05-10"))
        self.assertEqual(run("FREQ=YEARLY;BYMONTH=3;COUNT=2", "2026-01-15"),
                         ds("2026-03-15", "2027-03-15"))


class BoundsTests(unittest.TestCase):
    def test_until_inclusive_and_before_start(self):
        self.assertEqual(run("FREQ=DAILY;UNTIL=2026-01-03", "2026-01-01"),
                         ds("2026-01-01", "2026-01-02", "2026-01-03"))
        self.assertEqual(run("FREQ=DAILY;UNTIL=2025-12-31", "2026-01-01"), [])

    def test_impossible_rules_terminate_empty(self):
        self.assertEqual(run("FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=30;COUNT=3", "2024-02-29"), [])
        self.assertEqual(run("FREQ=DAILY;BYMONTH=4;BYMONTHDAY=31;COUNT=1", "2026-01-01"), [])
        self.assertEqual(list(occurrences(parse_rule("FREQ=MONTHLY;BYMONTH=2;BYMONTHDAY=31"), D("2026-01-01"))), [])

    def test_unbounded_expand_needs_a_limit(self):
        with self.assertRaises(RuleError):
            run("FREQ=DAILY", "2026-01-01")
        self.assertEqual(run("FREQ=DAILY", "2026-01-01", max_count=2), ds("2026-01-01", "2026-01-02"))
        self.assertEqual(run("FREQ=DAILY", "2026-01-01", end=D("2026-01-03")),
                         ds("2026-01-01", "2026-01-02", "2026-01-03"))
        self.assertEqual(run("FREQ=DAILY", "2026-01-01", max_count=0), [])
        self.assertEqual(run("FREQ=DAILY", "2026-01-05", end=D("2026-01-03")), [])

        self.assertEqual(run("FREQ=WEEKLY;COUNT=10", "2026-01-01", max_count=2), ds("2026-01-01", "2026-01-08"))
        self.assertEqual(run("FREQ=WEEKLY;COUNT=2", "2026-01-01", max_count=5), ds("2026-01-01", "2026-01-08"))
        self.assertEqual(run("FREQ=WEEKLY;COUNT=5", "2026-01-01", end=D("2026-01-14")), ds("2026-01-01", "2026-01-08"))

    def test_occurrences_is_lazy(self):
        gen = occurrences(parse_rule("FREQ=MONTHLY;BYMONTHDAY=15"), D("2026-01-20"))
        self.assertEqual(list(itertools.islice(gen, 3)), ds("2026-02-15", "2026-03-15", "2026-04-15"))

        dates = run("FREQ=YEARLY;INTERVAL=100", "2026-06-01", max_count=10)
        self.assertEqual(dates, ds("2026-06-01", "2126-06-01", "2226-06-01", "2326-06-01", "2426-06-01"))
