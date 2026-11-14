"""Command line interface: python -m recur COMMAND ..."""

import argparse
import sys

from .calfile import dump_schedule, load_schedule
from .dates import format_date, parse_date
from .describe import describe
from .errors import RecurError
from .expand import expand, occurrences
from .rule import parse_rule
from .schedule import Schedule


class _UsageError(Exception):
    pass


class _Parser(argparse.ArgumentParser):
    def error(self, message):
        raise _UsageError(message)


def _build_parser() -> argparse.ArgumentParser:
    parser = _Parser(prog="recur", add_help=False)
    sub = parser.add_subparsers(dest="command", required=True, parser_class=_Parser)
    p = sub.add_parser("expand", add_help=False)
    p.add_argument("rule")
    p.add_argument("--start", required=True)
    p.add_argument("--end")
    p.add_argument("--max", type=int)
    p = sub.add_parser("next", add_help=False)
    p.add_argument("rule")
    p.add_argument("--start", required=True)
    p.add_argument("--after", required=True)
    p = sub.add_parser("normalize", add_help=False)
    p.add_argument("rule")
    p = sub.add_parser("schedule", add_help=False)
    p.add_argument("--start", required=True)
    p.add_argument("--from", dest="first", required=True)
    p.add_argument("--to", dest="last", required=True)
    p.add_argument("--rule", action="append", default=[])
    p.add_argument("--add", action="append", default=[])
    p.add_argument("--exclude", action="append", default=[])
    p.add_argument("--exrule", action="append", default=[])
    p = sub.add_parser("file", add_help=False)
    p.add_argument("path")
    p.add_argument("--from", dest="first", required=True)
    p.add_argument("--to", dest="last", required=True)
    p = sub.add_parser("format", add_help=False)
    p.add_argument("path")
    p.add_argument("--comments", action="store_true")
    p = sub.add_parser("describe", add_help=False)
    p.add_argument("rule")
    p = sub.add_parser("explain", add_help=False)
    p.add_argument("path")
    return parser


def _read(path: str) -> str:
    try:
        with open(path, encoding="utf-8") as f:
            return f.read()
    except (OSError, UnicodeDecodeError) as exc:
        raise RecurError(f"cannot read {path}: {exc}") from None


def _run(args) -> list[str]:
    if args.command == "normalize":
        return [parse_rule(args.rule).to_text()]
    if args.command == "expand":
        end = parse_date(args.end) if args.end else None
        dates = expand(parse_rule(args.rule), parse_date(args.start), end=end, max_count=args.max)
        return [format_date(d) for d in dates]
    if args.command == "next":
        after = parse_date(args.after)
        for d in occurrences(parse_rule(args.rule), parse_date(args.start)):
            if d > after:
                return [format_date(d)]
        return ["none"]
    if args.command == "format":
        return dump_schedule(load_schedule(_read(args.path)), comments=args.comments).splitlines()
    if args.command == "describe":
        return [describe(parse_rule(args.rule))]
    if args.command == "explain":
        return load_schedule(_read(args.path)).describe()
    if args.command == "file":
        schedule = load_schedule(_read(args.path))
        dates = schedule.between(parse_date(args.first), parse_date(args.last))
        return [format_date(d) for d in dates]
    schedule = Schedule(parse_date(args.start))
    for text in args.rule:
        schedule.add_rule(text)
    for text in args.add:
        schedule.add_date(parse_date(text))
    for text in args.exclude:
        schedule.exclude(parse_date(text))
    for text in args.exrule:
        schedule.exclude_rule(text)
    dates = schedule.between(parse_date(args.first), parse_date(args.last))
    return [format_date(d) for d in dates]


def main(argv: list[str]) -> int:
    try:
        args = _build_parser().parse_args(argv)
        lines = _run(args)
    except _UsageError as exc:
        print(f"usage error: {exc}", file=sys.stderr)
        return 2
    except RecurError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2
    for line in lines:
        print(line, file=sys.stdout)
    return 0
