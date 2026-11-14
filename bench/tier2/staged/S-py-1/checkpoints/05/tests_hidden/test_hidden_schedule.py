import unittest
from datetime import date

from recur import RecurError, Schedule, parse_rule


class ScheduleTests(unittest.TestCase):
    def test_union_extras_exclusions_and_duplicates(self):
        s = Schedule(date(2026, 1, 1))
        s.add_rule("FREQ=WEEKLY;BYDAY=MO")
        s.add_rule(parse_rule("FREQ=MONTHLY;BYMONTHDAY=5"))
        s.add_date(date(2025, 12, 25))
        s.add_date(date(2026, 1, 7))
        s.exclude(date(2026, 1, 12))
        self.assertEqual(s.between(date(2025, 12, 1), date(2026, 1, 20)),
                         [date(2025, 12, 25), date(2026, 1, 5), date(2026, 1, 7), date(2026, 1, 19)])

    def test_count_applies_before_exclusions(self):
        s = Schedule(date(2026, 1, 1))
        s.add_rule("FREQ=DAILY;COUNT=3")
        s.exclude(date(2026, 1, 2))
        self.assertEqual(s.between(date(2026, 1, 1), date(2026, 1, 10)), [date(2026, 1, 1), date(2026, 1, 3)])

    def test_between_window_is_inclusive_and_checked(self):
        s = Schedule(date(2026, 1, 1))
        s.add_rule("FREQ=DAILY")
        self.assertEqual(s.between(date(2026, 3, 1), date(2026, 3, 1)), [date(2026, 3, 1)])
        with self.assertRaises(RecurError):
            s.between(date(2026, 3, 2), date(2026, 3, 1))
        with self.assertRaises(RecurError):
            s.add_rule("FREQ=NEVER")

    def test_next_after(self):
        s = Schedule(date(2026, 1, 1))
        s.add_rule("FREQ=DAILY;INTERVAL=7;COUNT=2")
        self.assertEqual(s.next_after(date(2025, 6, 1)), date(2026, 1, 1))
        self.assertEqual(s.next_after(date(2026, 1, 1)), date(2026, 1, 8))
        self.assertIsNone(s.next_after(date(2026, 1, 8)))
        s.add_date(date(2027, 6, 1))
        self.assertEqual(s.next_after(date(2026, 1, 8)), date(2027, 6, 1))

    def test_next_after_skips_exclusions_on_unbounded_rule(self):
        s = Schedule(date(2026, 1, 1))
        s.add_rule("FREQ=MONTHLY;BYDAY=-1FR")
        s.exclude(date(2026, 2, 27))
        self.assertEqual(s.next_after(date(2026, 1, 30)), date(2026, 3, 27))

        s = Schedule(date(2026, 1, 1))
        self.assertEqual(s.between(date(2026, 1, 1), date(2026, 12, 31)), [])
        self.assertIsNone(s.next_after(date(2026, 1, 1)))
