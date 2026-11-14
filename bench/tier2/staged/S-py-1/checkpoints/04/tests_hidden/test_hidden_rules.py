import unittest
from datetime import date

from recur import Rule, RuleError, parse_rule
from recur.dates import WEEKDAY_CODES, format_date, parse_date


class RuleParsingTests(unittest.TestCase):
    def test_defaults_and_fields(self):
        r = parse_rule("FREQ=DAILY")
        self.assertIsInstance(r, Rule)
        self.assertEqual(
            (r.freq, r.interval, r.count, r.until, r.bymonth, r.bymonthday, r.byday),
            ("DAILY", 1, None, None, (), (), ()),
        )

    def test_case_insensitive_and_sorted_fields(self):
        r = parse_rule("  byday=fr,-1mo,FR;freq=monthly;bymonth=12,3,3;bymonthday=-1,15  ")
        self.assertEqual(r.freq, "MONTHLY")
        self.assertEqual(r.byday, ((0, -1), (4, 0)))
        self.assertEqual(r.bymonth, (3, 12))
        self.assertEqual(r.bymonthday, (-1, 15))

    def test_equal_meaning_compares_equal(self):
        self.assertEqual(parse_rule("FREQ=WEEKLY;BYDAY=MO,WE;INTERVAL=1"),
                         parse_rule("byday=we,mo;freq=weekly"))
        self.assertNotEqual(parse_rule("FREQ=WEEKLY"), parse_rule("FREQ=WEEKLY;INTERVAL=2"))

    def test_to_text_spec_example(self):
        self.assertEqual(parse_rule("byday=fr,-1mo;freq=monthly").to_text(), "FREQ=MONTHLY;BYDAY=-1MO,FR")

        r = parse_rule("BYDAY=+3WE,1MO;UNTIL=2027-03-04;BYMONTHDAY=2;BYMONTH=6,1;INTERVAL=2;FREQ=YEARLY")
        self.assertEqual(r.to_text(),
                         "FREQ=YEARLY;INTERVAL=2;UNTIL=2027-03-04;BYMONTH=1,6;BYMONTHDAY=2;BYDAY=1MO,3WE")
        self.assertEqual(parse_rule("FREQ=DAILY;INTERVAL=1;COUNT=7").to_text(), "FREQ=DAILY;COUNT=7")

        self.assertEqual(parse_rule("FREQ=DAILY;UNTIL=2026-02-28").until, date(2026, 2, 28))

    def test_structural_errors(self):
        for text in ["", "   ", "FREQ=DAILY;", ";FREQ=DAILY", "FREQ", "FREQ=DAILY;COUNT",
                     "FREQ=DAILY;COLOR=RED", "FREQ=DAILY;FREQ=WEEKLY", "COUNT=3",
                     "FREQ=HOURLY", "FREQ=DAILY;;COUNT=2"]:
            with self.subTest(text=text):
                with self.assertRaises(RuleError):
                    parse_rule(text)

    def test_value_errors(self):
        for text in ["FREQ=DAILY;INTERVAL=0", "FREQ=DAILY;INTERVAL=x", "FREQ=DAILY;COUNT=-1",
                     "FREQ=DAILY;COUNT=1.5", "FREQ=DAILY;UNTIL=2026-02-30", "FREQ=DAILY;UNTIL=2026-2-1",
                     "FREQ=MONTHLY;BYMONTH=13", "FREQ=MONTHLY;BYMONTH=0", "FREQ=MONTHLY;BYMONTHDAY=0",
                     "FREQ=MONTHLY;BYMONTHDAY=32", "FREQ=MONTHLY;BYMONTHDAY=-32", "FREQ=MONTHLY;BYDAY=XX",
                     "FREQ=MONTHLY;BYDAY=6MO", "FREQ=MONTHLY;BYDAY=0MO", "FREQ=MONTHLY;BYDAY=-6FR",
                     "FREQ=MONTHLY;BYDAY=MO,,TU", "FREQ=MONTHLY;BYDAY=", "FREQ=MONTHLY;BYMONTH=1,"]:
            with self.subTest(text=text):
                with self.assertRaises(RuleError):
                    parse_rule(text)

    def test_combination_errors(self):
        for text in ["FREQ=DAILY;COUNT=2;UNTIL=2026-01-05", "FREQ=WEEKLY;BYMONTHDAY=3",
                     "FREQ=DAILY;BYDAY=1MO", "FREQ=WEEKLY;BYDAY=-1FR"]:
            with self.subTest(text=text):
                with self.assertRaises(RuleError):
                    parse_rule(text)
        # ordinals are fine for monthly and yearly
        parse_rule("FREQ=YEARLY;BYDAY=-1FR")
        parse_rule("FREQ=DAILY;BYMONTHDAY=-1")

        self.assertTrue(issubclass(RuleError, ValueError))


class DateTests(unittest.TestCase):
    def test_parse_and_format(self):
        self.assertEqual(parse_date("2024-02-29"), date(2024, 2, 29))
        self.assertEqual(format_date(date(5, 1, 2)), "0005-01-02")
        self.assertEqual(WEEKDAY_CODES, ("MO", "TU", "WE", "TH", "FR", "SA", "SU"))
        for text in ["2023-02-29", "2026-2-01", "26-02-01", "2026-02-01 ", "2026/02/01", "x"]:
            with self.subTest(text=text):
                with self.assertRaises(RuleError):
                    parse_date(text)
