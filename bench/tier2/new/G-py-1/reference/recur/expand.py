"""Expanding a rule into the dates it produces."""

from collections.abc import Iterator
from datetime import date, timedelta

from .dates import (add_months, days_in_month, horizon, month_index, nth_weekday,
                    resolve_monthday, week_start, weekdays_in_month)
from .errors import RuleError
from .rule import Rule


def _month_days(rule: Rule, year: int, month: int, start: date) -> list[int]:
    """Candidate day numbers in one month, by the month rule."""
    by_monthday = None
    if rule.bymonthday:
        by_monthday = {d for d in (resolve_monthday(year, month, n) for n in rule.bymonthday) if d}
    by_day = None
    if rule.byday:
        by_day = set()
        for weekday, n in rule.byday:
            if n == 0:
                by_day.update(weekdays_in_month(year, month, weekday))
            else:
                day = nth_weekday(year, month, weekday, n)
                if day is not None:
                    by_day.add(day)
    if by_monthday is None and by_day is None:
        return [start.day] if start.day <= days_in_month(year, month) else []
    if by_monthday is None:
        return sorted(by_day)
    if by_day is None:
        return sorted(by_monthday)
    return sorted(by_monthday & by_day)


def _passes_filters(rule: Rule, d: date) -> bool:
    if rule.bymonth and d.month not in rule.bymonth:
        return False
    if rule.bymonthday:
        if not any(resolve_monthday(d.year, d.month, n) == d.day for n in rule.bymonthday):
            return False
    if rule.byday and d.weekday() not in {wd for wd, _ in rule.byday}:
        return False
    return True


def _candidates(rule: Rule, start: date, last: date) -> Iterator[date]:
    """All candidates of active periods, ascending, possibly before start."""
    step = rule.interval
    if rule.freq == "DAILY":
        d = start
        while d <= last:
            if _passes_filters(rule, d):
                yield d
            d += timedelta(days=step)
    elif rule.freq == "WEEKLY":
        weekdays = sorted({wd for wd, _ in rule.byday}) or [start.weekday()]
        week = week_start(start)
        while week <= last:
            for wd in weekdays:
                d = week + timedelta(days=wd)
                if not rule.bymonth or d.month in rule.bymonth:
                    yield d
            week += timedelta(weeks=step)
    elif rule.freq == "MONTHLY":
        k = 0
        while True:
            year, month = add_months(start.year, start.month, k)
            if year * 12 + month - 1 > month_index(last):
                return
            if not rule.bymonth or month in rule.bymonth:
                for day in _month_days(rule, year, month, start):
                    yield date(year, month, day)
            k += step
    else:
        year = start.year
        months = rule.bymonth or (start.month,)
        while year <= last.year:
            for month in months:
                for day in _month_days(rule, year, month, start):
                    yield date(year, month, day)
            year += step


def occurrences(rule: Rule, start: date) -> Iterator[date]:
    last = horizon(start)
    if rule.until is not None:
        last = min(last, rule.until)
    produced = 0
    for d in _candidates(rule, start, last):
        if d < start:
            continue
        if d > last:
            return
        if rule.count is not None and produced >= rule.count:
            return
        produced += 1
        yield d


def expand(rule: Rule, start: date, *, end: date | None = None, max_count: int | None = None) -> list[date]:
    if rule.count is None and rule.until is None and end is None and max_count is None:
        raise RuleError("rule is unbounded: give COUNT, UNTIL, end or max_count")
    result: list[date] = []
    if max_count is not None and max_count <= 0:
        return result
    for d in occurrences(rule, start):
        if end is not None and d > end:
            break
        result.append(d)
        if max_count is not None and len(result) >= max_count:
            break
    return result
