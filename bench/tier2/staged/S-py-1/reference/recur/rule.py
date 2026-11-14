"""Parsing and canonical form of recurrence rules."""

import re
from dataclasses import dataclass
from datetime import date

from .dates import WEEKDAY_CODES, format_date, parse_date
from .errors import RecurError

FREQUENCIES = ("DAILY", "WEEKLY", "MONTHLY", "YEARLY")
KEYS = ("FREQ", "INTERVAL", "COUNT", "UNTIL", "BYMONTH", "BYMONTHDAY", "BYDAY")

_INT_RE = re.compile(r"^[+-]?\d+$")
_DAY_RE = re.compile(r"^([+-]?\d+)?([A-Z]{2})$")


@dataclass(frozen=True)
class Rule:
    freq: str
    interval: int = 1
    count: int | None = None
    until: date | None = None
    bymonth: tuple[int, ...] = ()
    bymonthday: tuple[int, ...] = ()
    byday: tuple[tuple[int, int], ...] = ()

    def to_text(self) -> str:
        parts = [f"FREQ={self.freq}"]
        if self.interval != 1:
            parts.append(f"INTERVAL={self.interval}")
        if self.count is not None:
            parts.append(f"COUNT={self.count}")
        if self.until is not None:
            parts.append(f"UNTIL={format_date(self.until)}")
        if self.bymonth:
            parts.append("BYMONTH=" + ",".join(str(m) for m in self.bymonth))
        if self.bymonthday:
            parts.append("BYMONTHDAY=" + ",".join(str(d) for d in self.bymonthday))
        if self.byday:
            items = [(str(n) if n else "") + WEEKDAY_CODES[wd] for wd, n in self.byday]
            parts.append("BYDAY=" + ",".join(items))
        return ";".join(parts)

    def describe(self) -> str:
        from .describe import describe
        return describe(self)


def _integer(key: str, text: str, low: int, high: int, allow_negative: bool = False) -> int:
    if not _INT_RE.match(text):
        raise RecurError(f"{key}: {text!r} is not an integer", key=key)
    value = int(text)
    if allow_negative and -high <= value <= -low:
        return value
    if not low <= value <= high:
        raise RecurError(f"{key}: {value} is out of range", key=key)
    return value


def _items(key: str, text: str) -> list[str]:
    items = [item.strip() for item in text.split(",")]
    if any(item == "" for item in items):
        raise RecurError(f"{key}: empty list item", key=key)
    return items


def _weekday(text: str) -> tuple[int, int]:
    m = _DAY_RE.match(text)
    if not m or m.group(2) not in WEEKDAY_CODES:
        raise RecurError(f"BYDAY: bad weekday {text!r}", key="BYDAY")
    ordinal = 0
    if m.group(1) is not None:
        ordinal = _integer("BYDAY", m.group(1), 1, 53, allow_negative=True)
    return WEEKDAY_CODES.index(m.group(2)), ordinal


def _until(text: str) -> date:
    try:
        return parse_date(text)
    except RecurError as exc:
        raise RecurError(f"UNTIL: {exc}", key="UNTIL") from None


def parse_rule(text: str) -> Rule:
    text = text.strip()
    if not text:
        raise RecurError("empty rule")
    values: dict[str, str] = {}
    for part in text.split(";"):
        part = part.strip()
        if part == "":
            raise RecurError("empty part in rule")
        if "=" not in part:
            raise RecurError(f"part {part!r} has no '='")
        key, value = part.split("=", 1)
        key, value = key.strip().upper(), value.strip().upper()
        if key not in KEYS:
            raise RecurError(f"unknown key {key}", key=key)
        if key in values:
            raise RecurError(f"key {key} given twice", key=key)
        values[key] = value
    if "FREQ" not in values:
        raise RecurError("FREQ is required", key="FREQ")
    freq = values["FREQ"]
    if freq not in FREQUENCIES:
        raise RecurError(f"unknown frequency {freq}", key="FREQ")
    if "COUNT" in values and "UNTIL" in values:
        raise RecurError("COUNT and UNTIL cannot both be given", key="UNTIL")

    interval = _integer("INTERVAL", values["INTERVAL"], 1, 10**9) if "INTERVAL" in values else 1
    count = _integer("COUNT", values["COUNT"], 1, 10**9) if "COUNT" in values else None
    until = _until(values["UNTIL"]) if "UNTIL" in values else None
    bymonth = ()
    if "BYMONTH" in values:
        bymonth = tuple(sorted({_integer("BYMONTH", v, 1, 12) for v in _items("BYMONTH", values["BYMONTH"])}))
    bymonthday = ()
    if "BYMONTHDAY" in values:
        if freq == "WEEKLY":
            raise RecurError("BYMONTHDAY is not allowed with FREQ=WEEKLY", key="BYMONTHDAY")
        items = _items("BYMONTHDAY", values["BYMONTHDAY"])
        bymonthday = tuple(sorted({_integer("BYMONTHDAY", v, 1, 31, allow_negative=True) for v in items}))
    byday = ()
    if "BYDAY" in values:
        pairs = {_weekday(v) for v in _items("BYDAY", values["BYDAY"])}
        if freq in ("DAILY", "WEEKLY") and any(n != 0 for _, n in pairs):
            raise RecurError(f"BYDAY ordinals are not allowed with FREQ={freq}", key="BYDAY")
        limit = 53 if freq == "YEARLY" and not bymonth else 5
        for _, n in pairs:
            if abs(n) > limit:
                raise RecurError(f"BYDAY: ordinal {n} is out of range", key="BYDAY")
        byday = tuple(sorted(pairs))
    return Rule(freq, interval, count, until, bymonth, bymonthday, byday)
