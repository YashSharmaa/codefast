"""Schedule files: a schedule written as NAME:VALUE lines."""

from datetime import date

from .dates import format_date, parse_date
from .describe import describe
from .errors import RecurError
from .rule import parse_rule
from .schedule import Schedule

NAMES = ("START", "RULE", "EXRULE", "ADD", "EXCLUDE")


def _entries(text: str) -> list[tuple[int, str, str]]:
    """(line number, NAME, value) for every line that is not blank or a comment."""
    entries = []
    for number, raw in enumerate(text.splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if ":" not in line:
            raise RecurError(f"line {number}: expected NAME:VALUE", line=number)
        name, value = line.split(":", 1)
        name, value = name.strip().upper(), value.strip()
        if name not in NAMES:
            raise RecurError(f"line {number}: unknown name {name}", line=number)
        entries.append((number, name, value))
    return entries


def _dates(value: str) -> list[date]:
    return [parse_date(item.strip()) for item in value.split(",")]


def load_schedule(text: str) -> Schedule:
    """Read a schedule file. Errors about one line start with "line N: "."""
    entries = _entries(text)
    starts = [(number, value) for number, name, value in entries if name == "START"]
    if not starts:
        raise RecurError("missing START line")
    if len(starts) > 1:
        raise RecurError(f"line {starts[1][0]}: START given twice", line=starts[1][0])
    number = starts[0][0]
    try:
        schedule = Schedule(parse_date(starts[0][1]))
        for number, name, value in entries:
            if name == "RULE":
                schedule.add_rule(parse_rule(value))
            elif name == "EXRULE":
                schedule.exclude_rule(parse_rule(value))
            elif name == "ADD":
                for d in _dates(value):
                    schedule.add_date(d)
            elif name == "EXCLUDE":
                for d in _dates(value):
                    schedule.exclude(d)
    except RecurError as exc:
        raise RecurError(f"line {number}: {exc}", key=exc.key, line=number) from None
    return schedule


def dump_schedule(schedule: Schedule, *, comments: bool = False) -> str:
    """The canonical text of a schedule file, which load_schedule reads back."""
    lines = [f"START:{format_date(schedule.start)}"]
    for name, rules in (("RULE", schedule.rules), ("EXRULE", schedule.exclusion_rules)):
        for rule in rules:
            if comments:
                lines.append(f"# {describe(rule)}")
            lines.append(f"{name}:{rule.to_text()}")
    lines += [f"ADD:{format_date(d)}" for d in sorted(schedule.extra)]
    lines += [f"EXCLUDE:{format_date(d)}" for d in sorted(schedule.excluded)]
    return "".join(line + "\n" for line in lines)
