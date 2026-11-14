"""Command line interface: python -m csvq SQL --table NAME=PATH ..."""

import argparse
import sys

from .engine import query
from .errors import CsvqError
from .table import Table


class _UsageError(Exception):
    pass


class _Parser(argparse.ArgumentParser):
    def error(self, message):
        raise _UsageError(message)


def _parse_args(argv: list[str]):
    parser = _Parser(prog="csvq", add_help=False)
    parser.add_argument("sql")
    parser.add_argument("--table", action="append", required=True)
    args = parser.parse_args(argv)
    specs = []
    for spec in args.table:
        name, sep, path = spec.partition("=")
        if not sep or not name or not path:
            raise _UsageError(f"--table needs NAME=PATH, got {spec!r}")
        specs.append((name, path))
    return args.sql, specs


def main(argv: list[str]) -> int:
    try:
        sql, specs = _parse_args(argv)
    except _UsageError as exc:
        print(f"usage: csvq SQL --table NAME=PATH ...: {exc}", file=sys.stderr)
        return 2
    try:
        tables = {}
        for name, path in specs:
            with open(path, encoding="utf-8", newline="") as handle:
                tables[name] = Table.from_csv(handle.read())
        text = query(sql, tables).to_csv()
    except (CsvqError, OSError) as exc:
        message = str(exc).replace("\n", " ")
        print(f"error: {message}", file=sys.stderr)
        return 1
    sys.stdout.write(text)
    return 0
