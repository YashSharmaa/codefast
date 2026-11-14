"""Command line interface: python -m recur COMMAND ..."""

import argparse
import sys

from .dates import format_date, parse_date
from .errors import RuleError
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
    return parser


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
    schedule = Schedule(parse_date(args.start))
    for text in args.rule:
        schedule.add_rule(text)
    for text in args.add:
        schedule.add_date(parse_date(text))
    for text in args.exclude:
        schedule.exclude(parse_date(text))
    dates = schedule.between(parse_date(args.first), parse_date(args.last))
    return [format_date(d) for d in dates]


def main(argv: list[str]) -> int:
    try:
        args = _build_parser().parse_args(argv)
        lines = _run(args)
    except _UsageError as exc:
        print(f"usage error: {exc}", file=sys.stderr)
        return 2
    except RuleError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2
    for line in lines:
        print(line, file=sys.stdout)
    return 0
