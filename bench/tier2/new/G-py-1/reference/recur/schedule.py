"""Schedules: several rules plus extra and excluded dates."""

import heapq
from datetime import date

from .errors import RuleError
from .expand import occurrences
from .rule import Rule, parse_rule


class Schedule:
    def __init__(self, start: date):
        self.start = start
        self.rules: list[Rule] = []
        self.extra: set[date] = set()
        self.excluded: set[date] = set()

    def add_rule(self, rule: Rule | str) -> None:
        if isinstance(rule, str):
            rule = parse_rule(rule)
        self.rules.append(rule)

    def add_date(self, d: date) -> None:
        self.extra.add(d)

    def exclude(self, d: date) -> None:
        self.excluded.add(d)

    def _merged(self):
        """All schedule dates in ascending order, without duplicates or exclusions."""
        streams = [occurrences(rule, self.start) for rule in self.rules]
        streams.append(iter(sorted(self.extra)))
        previous = None
        for d in heapq.merge(*streams):
            if d == previous or d in self.excluded:
                continue
            previous = d
            yield d

    def between(self, first: date, last: date) -> list[date]:
        if first > last:
            raise RuleError("first date is after last date")
        result = []
        for d in self._merged():
            if d > last:
                break
            if d >= first:
                result.append(d)
        return result

    def next_after(self, d: date) -> date | None:
        for candidate in self._merged():
            if candidate > d:
                return candidate
        return None
