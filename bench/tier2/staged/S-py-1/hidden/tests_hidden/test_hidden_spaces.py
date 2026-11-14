import contextlib
import io
import unittest
from datetime import date

from recur import RecurError, Schedule, dump_schedule, expand, load_schedule, parse_rule
from recur.cli import main

CANONICAL = "FREQ=MONTHLY;BYDAY=-1FR,MO;COUNT=3"


def key_of(text):
    try:
        parse_rule(text)
    except RecurError as exc:
        return exc.key
    raise AssertionError(f"no RecurError for {text!r}")


def call(*argv):
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        code = main(list(argv))
    return code, out.getvalue(), err.getvalue()


class SpacesAcceptedTests(unittest.TestCase):
    def test_reported_example(self):
        self.assertEqual(parse_rule("FREQ=WEEKLY; BYDAY=MO, FR"), parse_rule("FREQ=WEEKLY;BYDAY=FR,MO"))

    def test_spaces_after_separators(self):
        self.assertEqual(parse_rule("FREQ=MONTHLY; BYDAY=-1FR,MO; COUNT=3"), parse_rule(CANONICAL))

    def test_spaces_around_keys_values_and_items(self):
        rule = parse_rule("FREQ = MONTHLY ; BYDAY = -1FR , MO ; COUNT = 3")
        self.assertEqual(rule, parse_rule(CANONICAL))
        self.assertEqual(rule.to_text(), "FREQ=MONTHLY;COUNT=3;BYDAY=MO,-1FR")

    def test_tabs(self):
        self.assertEqual(parse_rule("freq=monthly;\tbyday=mo,\t-1fr;count=3"), parse_rule(CANONICAL))
        self.assertEqual(parse_rule("FREQ\t=\tDAILY\t;\tINTERVAL = 2"), parse_rule("FREQ=DAILY;INTERVAL=2"))

    def test_every_list_and_until(self):
        rule = parse_rule(" FREQ=YEARLY ; BYMONTH = 6 , 1 ; BYMONTHDAY = -1 ,15 ; UNTIL = 2030-01-01 ")
        self.assertEqual((rule.bymonth, rule.bymonthday, rule.until), ((1, 6), (-1, 15), date(2030, 1, 1)))
        self.assertEqual(rule.to_text(), "FREQ=YEARLY;UNTIL=2030-01-01;BYMONTH=1,6;BYMONTHDAY=-1,15")

    def test_expand_and_schedules_with_spaces(self):
        self.assertEqual(expand(parse_rule("FREQ = MONTHLY ; BYDAY = -1FR ; COUNT = 2"), date(2026, 1, 1)),
                         [date(2026, 1, 30), date(2026, 2, 27)])
        s = Schedule(date(2026, 1, 1))
        s.add_rule("FREQ=WEEKLY; BYDAY=MO")
        s.exclude_rule("FREQ=MONTHLY; BYDAY = 1MO")
        self.assertEqual(s.between(date(2026, 1, 1), date(2026, 1, 31)),
                         [date(2026, 1, 12), date(2026, 1, 19), date(2026, 1, 26)])

    def test_schedule_file_with_spaces(self):
        s = load_schedule("START:2026-01-01\nRULE: FREQ=DAILY ; COUNT = 2\nEXRULE: FREQ=WEEKLY ; BYDAY = SA, SU\n")
        self.assertEqual(dump_schedule(s),
                         "START:2026-01-01\nRULE:FREQ=DAILY;COUNT=2\nEXRULE:FREQ=WEEKLY;BYDAY=SA,SU\n")

    def test_cli_with_spaces(self):
        self.assertEqual(call("normalize", "freq = weekly ; byday = we , mo"), (0, "FREQ=WEEKLY;BYDAY=MO,WE\n", ""))
        self.assertEqual(call("describe", "FREQ=WEEKLY; BYDAY=WE, MO"), (0, "every week on Monday and Wednesday\n", ""))
        self.assertEqual(call("expand", "FREQ=DAILY; COUNT=2", "--start", "2026-01-01"), (0, "2026-01-01\n2026-01-02\n", ""))


class SpacesStillRejectedTests(unittest.TestCase):
    def test_whitespace_inside_a_token(self):
        cases = [("FREQ=DAILY;BY DAY=MO", "BY DAY"), ("FREQ=DAILY;COUNT=1 0", "COUNT"),
                 ("FREQ=MONTHLY;BYDAY=2 TU", "BYDAY"), ("FREQ=DAILY;UNTIL=2026-01- 05", "UNTIL"),
                 ("FREQ=MONTHLY;BYMONTHDAY=1 5", "BYMONTHDAY")]
        for text, key in cases:
            with self.subTest(text=text):
                self.assertEqual(key_of(text), key)

    def test_empty_parts_values_and_items(self):
        cases = [("FREQ=DAILY; ;COUNT=2", None), ("FREQ=DAILY;\t;COUNT=2", None), (" FREQ=DAILY ; ", None),
                 ("FREQ=DAILY;COUNT= ;INTERVAL=2", "COUNT"), ("FREQ=WEEKLY;BYDAY=MO, ,FR", "BYDAY"),
                 ("FREQ=MONTHLY;BYMONTH=1, ", "BYMONTH")]
        for text, key in cases:
            with self.subTest(text=text):
                self.assertEqual(key_of(text), key)

    def test_unknown_and_repeated_keys_without_whitespace(self):
        self.assertEqual(key_of("FREQ=DAILY; COLOR = red"), "COLOR")
        self.assertEqual(key_of("FREQ=DAILY; COUNT=2 ; count = 3"), "COUNT")
        self.assertEqual(key_of("  ; FREQ=DAILY"), None)
