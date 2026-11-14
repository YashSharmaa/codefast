# recur: a recurring-date rule expander

`recur` turns compact recurrence rules such as `FREQ=MONTHLY;BYDAY=-1FR;COUNT=6` into concrete
calendar dates. It is a Python package with a small command line. It works on whole dates only
(no times, no time zones). The project will be checked by tests written against the interface
below, so names, signatures, error types and output formats must match exactly.

## Package layout

All code lives in the package `recur/` at the project root.

| Module | Public names |
|---|---|
| `recur/errors.py` | `class RuleError(ValueError)`, the only error type the package raises on bad input |
| `recur/dates.py` | `parse_date(text: str) -> datetime.date`, `format_date(d: datetime.date) -> str`, `WEEKDAY_CODES` |
| `recur/rule.py` | `Rule`, `parse_rule(text: str) -> Rule` |
| `recur/expand.py` | `occurrences(rule: Rule, start: date) -> Iterator[date]`, `expand(rule, start, *, end=None, max_count=None) -> list[date]` |
| `recur/schedule.py` | `class Schedule` |
| `recur/cli.py` | `main(argv: list[str]) -> int` |
| `recur/__main__.py` | makes `python -m recur ...` run `main(sys.argv[1:])` and exit with its result |

`recur/__init__.py` re-exports `RuleError`, `Rule`, `parse_rule`, `occurrences`, `expand` and `Schedule`.

## Dates

`parse_date` accepts exactly `YYYY-MM-DD` (four digits, two, two) naming a real date and raises
`RuleError` otherwise. `format_date` writes the same form. `WEEKDAY_CODES` is the tuple
`("MO", "TU", "WE", "TH", "FR", "SA", "SU")`, index 0 is Monday as in `date.weekday()`.

## Rules

`parse_rule` reads `KEY=VALUE` parts separated by `;`. Leading and trailing whitespace of the
whole text is ignored. Keys and values are case-insensitive. Keys:

| Key | Value | Default |
|---|---|---|
| `FREQ` | `DAILY`, `WEEKLY`, `MONTHLY` or `YEARLY`. Required. | |
| `INTERVAL` | integer of at least 1 | 1 |
| `COUNT` | integer of at least 1 | none |
| `UNTIL` | a date as `YYYY-MM-DD`, inclusive | none |
| `BYMONTH` | comma list of integers 1 to 12 | empty |
| `BYMONTHDAY` | comma list of integers 1 to 31 or -31 to -1 (-1 is the last day of the month) | empty |
| `BYDAY` | comma list of weekday codes, each with an optional ordinal 1 to 5 or -1 to -5 in front, as in `MO`, `2TU`, `-1FR`, `+3WE` | empty |

`RuleError` is raised for: empty text, an empty part (so a trailing `;` is an error), a part
without `=`, an unknown key, a key given twice, a missing `FREQ`, an unknown frequency, a
non-integer or out-of-range number, an empty value or list item, a bad weekday code, both `COUNT` and
`UNTIL`, `BYMONTHDAY` with `FREQ=WEEKLY`, and a `BYDAY` ordinal with `FREQ=DAILY` or `FREQ=WEEKLY`.

`Rule` is a frozen dataclass with these fields, so two rules with the same meaning compare equal:

- `freq: str` in upper case, `interval: int`, `count: int | None`, `until: date | None`
- `bymonth: tuple[int, ...]` and `bymonthday: tuple[int, ...]`, sorted ascending, no duplicates
- `byday: tuple[tuple[int, int], ...]`, pairs `(weekday, ordinal)` with weekday 0 to 6 and
  ordinal 0 when none was given, sorted ascending, no duplicates

`Rule.to_text() -> str` gives the canonical text: keys in the order `FREQ`, `INTERVAL`, `COUNT`,
`UNTIL`, `BYMONTH`, `BYMONTHDAY`, `BYDAY`, keys at their default left out (`INTERVAL=1` is never
written), upper case, lists in the field order above, ordinals without a `+`. Example:
`parse_rule("byday=fr,-1mo;freq=monthly").to_text() == "FREQ=MONTHLY;BYDAY=-1MO,FR"`.

## Which dates a rule produces

A rule is expanded from a start date. Periods are counted from the period that contains the
start: period 0, `INTERVAL`, `2*INTERVAL` and so on are active, the others are skipped.

- `DAILY`: each active day is a candidate if it passes every given filter: its month is in
  `BYMONTH`, its day matches `BYMONTHDAY` (negative values count from the end of the month),
  its weekday is in `BYDAY`.
- `WEEKLY`: weeks run Monday to Sunday. In an active week the candidates are the days whose
  weekday is in `BYDAY`, or the start's weekday when `BYDAY` is empty, then filtered by `BYMONTH`.
- `MONTHLY`: in an active month (skipped entirely when `BYMONTH` is given and does not hold
  it) the candidates are found by the month rule below.
- `YEARLY`: in an active year, for each month in `BYMONTH` (or the start's month when
  `BYMONTH` is empty), the candidates are found by the month rule below.

The month rule: with `BYMONTHDAY` only, the listed days (a day the month does not have, such
as 31 in April, is skipped). With `BYDAY` only, every listed weekday without an ordinal, and
for an ordinal n the n-th such weekday of the month (n < 0 counts from the end, a fifth one
that does not exist is skipped). With both, only days that match both lists. With neither,
the start's day of the month, skipped in months without it (so a 31st start skips short
months and a 29 February start produces only leap years).

The occurrences are all candidates that are on or after the start, in ascending order. The
start is an occurrence only when it is a candidate itself. `UNTIL` drops everything after it.
`COUNT` keeps only the first `COUNT` occurrences. No date later than the start plus 400 years
(same month and day) is ever produced, so a rule that can never match, such as
`FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=30`, yields nothing and terminates.

`occurrences` yields the occurrences lazily in ascending order. `expand` returns them as a list,
also stopping after `end` (inclusive) and after `max_count` dates when those are given. If the
rule has neither `COUNT` nor `UNTIL` and both `end` and `max_count` are `None`, `expand` raises
`RuleError`.

## Schedules

`Schedule(start: date)` combines rules with extra and excluded dates.

- `add_rule(rule: Rule | str) -> None`: a string is parsed with `parse_rule`.
- `add_date(d: date) -> None`: an extra date, included even if it is before the start.
- `exclude(d: date) -> None`: a date that is never produced, whatever produced it.
- `between(first: date, last: date) -> list[date]`: every date of the schedule from `first` to
  `last` inclusive, ascending, without duplicates. Each rule is expanded from the schedule's
  start, and its `COUNT` counts occurrences before exclusions are removed. `RuleError` if
  `first > last`.
- `next_after(d: date) -> date | None`: the first schedule date strictly after `d`, or `None`.

## Command line

`main(argv)` writes to `sys.stdout` and `sys.stderr` as they are at call time and returns the
exit status without calling `sys.exit`. Output dates are `YYYY-MM-DD`, one per line. Dates given
as options are read with `parse_date`, and `--end` and `--max` are passed to `expand` as `end`
and `max_count`.

```
expand RULE --start DATE [--end DATE] [--max N]      the dates of expand(...)
next RULE --start DATE --after DATE                  the first occurrence strictly after DATE, or the word none
normalize RULE                                       the canonical text of the rule
schedule --start DATE --from DATE --to DATE [--rule RULE]... [--add DATE]... [--exclude DATE]...
                                                     the dates of Schedule.between(from, to)
```

Exit status 0 on success. On a `RuleError` it prints `error: <message>` as one line to stderr
and returns 2. A usage error (no command, an unknown command, a missing option, a non-integer `--max`) prints
a message to stderr and returns 2.

## Constraints

Python 3.15, standard library only, no third-party packages. Put your own tests anywhere except
`tests_hidden/`, which is reserved for the checking tests and will be replaced.
