"""Schedules: several rules plus extra and excluded dates."""

import heapq
from datetime import date

from .dates import format_date
from .errors import RecurError
from .expand import occurrences
from .rule import Rule, parse_rule


def _blocked(d: date, blockers: list, heads: list) -> bool:
    """Whether an exclusion rule produces d. heads[i] is the next date of blockers[i] not yet passed."""
    blocked = False
    for i, blocker in enumerate(blockers):
        while heads[i] is not None and heads[i] < d:
            heads[i] = next(blocker, None)
        if heads[i] == d:
            blocked = True
    return blocked


class Schedule:
    def __init__(self, start: date):
        self.start = start
        self.rules: list[Rule] = []
        self.extra: set[date] = set()
        self.excluded: set[date] = set()
        self.exclusion_rules: list[Rule] = []

    def add_rule(self, rule: Rule | str) -> None:
        if isinstance(rule, str):
            rule = parse_rule(rule)
        self.rules.append(rule)

    def add_date(self, d: date) -> None:
        self.extra.add(d)

    def exclude(self, d: date) -> None:
        self.excluded.add(d)

    def exclude_rule(self, rule: Rule | str) -> None:
        """Every occurrence of the rule, expanded from the start, is left out of the schedule."""
        if isinstance(rule, str):
            rule = parse_rule(rule)
        self.exclusion_rules.append(rule)

    def _merged(self):
        """All schedule dates in ascending order, without duplicates or exclusions."""
        streams = [occurrences(rule, self.start) for rule in self.rules]
        streams.append(iter(sorted(self.extra)))
        blockers = [occurrences(rule, self.start) for rule in self.exclusion_rules]
        heads = [next(b, None) for b in blockers]
        previous = None
        for d in heapq.merge(*streams):
            if d == previous or d in self.excluded:
                continue
            previous = d
            if _blocked(d, blockers, heads):
                continue
            yield d

    def describe(self) -> list[str]:
        """One line per rule, exclusion rule, extra date and excluded date."""
        from .describe import describe
        lines = [describe(rule) for rule in self.rules]
        lines += ["except " + describe(rule) for rule in self.exclusion_rules]
        lines += ["also on " + format_date(d) for d in sorted(self.extra)]
        lines += ["except on " + format_date(d) for d in sorted(self.excluded)]
        return lines

    def between(self, first: date, last: date) -> list[date]:
        if first > last:
            raise RecurError("first date is after last date")
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
