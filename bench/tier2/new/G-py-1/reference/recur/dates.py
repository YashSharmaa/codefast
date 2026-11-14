"""Date parsing and calendar helpers."""

import calendar
import re
from datetime import date, timedelta

from .errors import RuleError

WEEKDAY_CODES = ("MO", "TU", "WE", "TH", "FR", "SA", "SU")

_DATE_RE = re.compile(r"^(\d{4})-(\d{2})-(\d{2})$")


def parse_date(text: str) -> date:
    m = _DATE_RE.match(text) if isinstance(text, str) else None
    if not m:
        raise RuleError(f"bad date {text!r}: expected YYYY-MM-DD")
    try:
        return date(int(m.group(1)), int(m.group(2)), int(m.group(3)))
    except ValueError:
        raise RuleError(f"bad date {text!r}: no such day") from None


def format_date(d: date) -> str:
    return f"{d.year:04d}-{d.month:02d}-{d.day:02d}"


def days_in_month(year: int, month: int) -> int:
    return calendar.monthrange(year, month)[1]


def add_months(year: int, month: int, k: int) -> tuple[int, int]:
    index = year * 12 + (month - 1) + k
    return index // 12, index % 12 + 1


def month_index(d: date) -> int:
    return d.year * 12 + d.month - 1


def week_start(d: date) -> date:
    return d - timedelta(days=d.weekday())


def resolve_monthday(year: int, month: int, n: int) -> int | None:
    """Day number for BYMONTHDAY value n in the month, or None if it does not exist."""
    last = days_in_month(year, month)
    day = n if n > 0 else last + 1 + n
    if 1 <= day <= last:
        return day
    return None


def nth_weekday(year: int, month: int, weekday: int, n: int) -> int | None:
    """Day number of the n-th weekday of the month (n < 0 from the end), or None."""
    days = weekdays_in_month(year, month, weekday)
    if n > 0:
        return days[n - 1] if n <= len(days) else None
    return days[n] if -n <= len(days) else None


def weekdays_in_month(year: int, month: int, weekday: int) -> list[int]:
    first = date(year, month, 1).weekday()
    day = 1 + (weekday - first) % 7
    last = days_in_month(year, month)
    return list(range(day, last + 1, 7))


def horizon(start: date) -> date:
    """The last date ever produced for a start: the same day 400 years later."""
    return date(start.year + 400, start.month, start.day) if start.year + 400 <= 9999 else date.max
