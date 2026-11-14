import contextlib
import io
import os
import tempfile
import unittest
from datetime import date

import recur
from recur import Schedule, describe, dump_schedule, load_schedule, parse_rule
from recur.cli import main
from recur.describe import ordinal


def say(text):
    return describe(parse_rule(text))


def call(*argv):
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        code = main(list(argv))
    return code, out.getvalue(), err.getvalue()


def sample_schedule():
    s = Schedule(date(2026, 1, 1))
    s.add_rule("FREQ=WEEKLY;BYDAY=MO")
    s.add_rule("FREQ=MONTHLY;BYMONTHDAY=-1;COUNT=3")
    s.exclude_rule("FREQ=MONTHLY;BYDAY=1MO")
    s.add_date(date(2026, 1, 21))
    s.add_date(date(2026, 1, 20))
    s.add_date(date(2026, 1, 21))
    s.exclude(date(2026, 1, 19))
    return s


class OrdinalTests(unittest.TestCase):
    def test_ordinal_suffixes(self):
        got = [ordinal(n) for n in (1, 2, 3, 4, 10, 11, 12, 13, 14, 21, 22, 23, 31, 53, 101, 111, 112, 113, 122)]
        self.assertEqual(got, ["1st", "2nd", "3rd", "4th", "10th", "11th", "12th", "13th", "14th", "21st", "22nd",
                               "23rd", "31st", "53rd", "101st", "111th", "112th", "113th", "122nd"])


class DescribeTests(unittest.TestCase):
    def test_frequency_and_interval(self):
        self.assertEqual(say("FREQ=DAILY"), "every day")
        self.assertEqual(say("FREQ=WEEKLY;INTERVAL=1"), "every week")
        self.assertEqual(say("FREQ=MONTHLY;INTERVAL=2"), "every other month")
        self.assertEqual(say("FREQ=DAILY;INTERVAL=3"), "every 3 days")
        self.assertEqual(say("FREQ=YEARLY;INTERVAL=10"), "every 10 years")

    def test_month_names(self):
        self.assertEqual(say("FREQ=MONTHLY;BYMONTH=3"), "every month in March")
        self.assertEqual(say("FREQ=DAILY;BYMONTH=12,1"), "every day in January and December")
        self.assertEqual(say("FREQ=YEARLY;BYMONTH=9,6,2,11"), "every year in February, June, September and November")

    def test_month_days(self):
        self.assertEqual(say("FREQ=MONTHLY;BYMONTHDAY=22"), "every month on the 22nd")
        self.assertEqual(say("FREQ=MONTHLY;BYMONTHDAY=-1,15,1,-3"),
                         "every month on the 1st, 15th, 3rd to last day and last day")
        self.assertEqual(say("FREQ=DAILY;BYMONTHDAY=-1"), "every day on the last day")

    def test_weekdays_and_ordinals(self):
        self.assertEqual(say("FREQ=WEEKLY;INTERVAL=2;BYDAY=SU,TU,TH"), "every other week on Tuesday, Thursday and Sunday")
        self.assertEqual(say("FREQ=MONTHLY;BYDAY=-2MO,-1MO,1MO"),
                         "every month on the 2nd to last Monday, the last Monday and the 1st Monday")
        self.assertEqual(say("FREQ=MONTHLY;BYDAY=SU,3WE,MO"), "every month on Monday, the 3rd Wednesday and Sunday")
        self.assertEqual(say("FREQ=DAILY;BYDAY=SA"), "every day on Saturday")

    def test_month_days_and_weekdays_together(self):
        self.assertEqual(say("FREQ=MONTHLY;BYMONTHDAY=13;BYDAY=FR"), "every month on the 13th when it is Friday")
        self.assertEqual(say("FREQ=YEARLY;BYMONTH=5;BYMONTHDAY=7,1,2,3,4,5,6;BYDAY=MO"),
                         "every year in May on the 1st, 2nd, 3rd, 4th, 5th, 6th and 7th when it is Monday")
        self.assertEqual(say("FREQ=MONTHLY;BYMONTHDAY=-1;BYDAY=-1FR,SA"),
                         "every month on the last day when it is the last Friday and Saturday")

    def test_count_and_until(self):
        self.assertEqual(say("FREQ=DAILY;COUNT=1"), "every day, once")
        self.assertEqual(say("FREQ=WEEKLY;COUNT=12;BYDAY=WE"), "every week on Wednesday, 12 times")
        self.assertEqual(say("FREQ=YEARLY;BYMONTH=11;BYDAY=4TH;UNTIL=2030-12-31"),
                         "every year in November on the 4th Thursday, until 2030-12-31")
        self.assertEqual(say("FREQ=MONTHLY;INTERVAL=3;BYMONTHDAY=-1,1,-2;COUNT=4"),
                         "every 3 months on the 1st, 2nd to last day and last day, 4 times")

    def test_rule_method_and_export(self):
        rule = parse_rule("FREQ=WEEKLY;BYDAY=FR,MO")
        self.assertEqual(rule.describe(), "every week on Monday and Friday")
        self.assertEqual(rule.describe(), describe(rule))
        self.assertIs(recur.describe, describe)


class ScheduleDescribeTests(unittest.TestCase):
    def test_schedule_lines(self):
        self.assertEqual(sample_schedule().describe(), [
            "every week on Monday",
            "every month on the last day, 3 times",
            "except every month on the 1st Monday",
            "also on 2026-01-20",
            "also on 2026-01-21",
            "except on 2026-01-19",
        ])
        self.assertEqual(Schedule(date(2026, 1, 1)).describe(), [])

    def test_dump_with_comments(self):
        s = sample_schedule()
        self.assertEqual(dump_schedule(s, comments=True),
                         "START:2026-01-01\n"
                         "# every week on Monday\nRULE:FREQ=WEEKLY;BYDAY=MO\n"
                         "# every month on the last day, 3 times\nRULE:FREQ=MONTHLY;COUNT=3;BYMONTHDAY=-1\n"
                         "# every month on the 1st Monday\nEXRULE:FREQ=MONTHLY;BYDAY=1MO\n"
                         "ADD:2026-01-20\nADD:2026-01-21\nEXCLUDE:2026-01-19\n")
        plain = dump_schedule(s)
        self.assertNotIn("#", plain)
        self.assertEqual(plain, dump_schedule(s, comments=False))
        again = load_schedule(dump_schedule(s, comments=True))
        self.assertEqual(again.between(date(2026, 1, 1), date(2026, 3, 31)), s.between(date(2026, 1, 1), date(2026, 3, 31)))


class DescribeCliTests(unittest.TestCase):
    def test_describe_command(self):
        self.assertEqual(call("describe", "byday=-1fr;freq=monthly;count=2"),
                         (0, "every month on the last Friday, 2 times\n", ""))
        code, out, err = call("describe", "FREQ=MONTHLY;BYDAY=0MO")
        self.assertEqual((code, out), (2, ""))
        self.assertTrue(err.startswith("error: "))

    def test_explain_and_format_with_comments(self):
        s = sample_schedule()
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "s.sched")
            with open(path, "w", encoding="utf-8") as f:
                f.write(dump_schedule(s))
            code, out, err = call("explain", path)
            self.assertEqual((code, err), (0, ""))
            self.assertEqual(out.splitlines(), s.describe())
            self.assertEqual(call("format", path, "--comments"), (0, dump_schedule(s, comments=True), ""))
            self.assertEqual(call("format", path), (0, dump_schedule(s), ""))
            code, out, err = call("explain", os.path.join(tmp, "nope.sched"))
            self.assertEqual((code, out), (2, ""))
            self.assertTrue(err.startswith("error: "))
