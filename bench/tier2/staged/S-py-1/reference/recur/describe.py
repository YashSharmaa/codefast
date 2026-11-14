"""Plain English descriptions of rules."""

from __future__ import annotations

from typing import TYPE_CHECKING

from .dates import format_date

if TYPE_CHECKING:
    from .rule import Rule

MONTH_NAMES = ("January", "February", "March", "April", "May", "June", "July",
               "August", "September", "October", "November", "December")
WEEKDAY_NAMES = ("Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday")
UNITS = {"DAILY": "day", "WEEKLY": "week", "MONTHLY": "month", "YEARLY": "year"}


def ordinal(n: int) -> str:
    """1st, 2nd, 3rd, 4th, ... 11th, 12th, 13th, ... 21st, 22nd, ..."""
    if 11 <= n % 100 <= 13:
        suffix = "th"
    else:
        suffix = {1: "st", 2: "nd", 3: "rd"}.get(n % 10, "th")
    return f"{n}{suffix}"


def join_list(items: list[str]) -> str:
    """A, A and B, A, B and C."""
    if len(items) == 1:
        return items[0]
    return ", ".join(items[:-1]) + " and " + items[-1]


def _from_end(n: int) -> str:
    return "last" if n == -1 else f"{ordinal(-n)} to last"


def _monthday(n: int) -> str:
    return ordinal(n) if n > 0 else _from_end(n) + " day"


def _weekday(weekday: int, n: int) -> str:
    name = WEEKDAY_NAMES[weekday]
    if n == 0:
        return name
    return f"the {ordinal(n) if n > 0 else _from_end(n)} {name}"


def describe(rule: Rule) -> str:
    """The rule in plain English, such as "every other week on Monday and Friday, 6 times"."""
    unit = UNITS[rule.freq]
    if rule.interval == 1:
        text = f"every {unit}"
    elif rule.interval == 2:
        text = f"every other {unit}"
    else:
        text = f"every {rule.interval} {unit}s"
    if rule.bymonth:
        text += " in " + join_list([MONTH_NAMES[m - 1] for m in rule.bymonth])
    if rule.bymonthday:
        days = [n for n in rule.bymonthday if n > 0] + [n for n in rule.bymonthday if n < 0]
        text += " on the " + join_list([_monthday(n) for n in days])
    if rule.byday:
        text += " when it is " if rule.bymonthday else " on "
        text += join_list([_weekday(weekday, n) for weekday, n in rule.byday])
    if rule.count is not None:
        text += ", once" if rule.count == 1 else f", {rule.count} times"
    if rule.until is not None:
        text += f", until {format_date(rule.until)}"
    return text
