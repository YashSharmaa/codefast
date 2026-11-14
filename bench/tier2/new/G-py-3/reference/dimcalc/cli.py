"""Command line interface: python -m dimcalc COMMAND ..."""

import re
import sys

from .errors import DimcalcError
from .parse import parse_unit
from .quantity import Quantity, parse_quantity, split

_DECIMAL_RE = re.compile(r"[+-]?([0-9]+(\.[0-9]*)?|\.[0-9]+)([eE][+-]?[0-9]+)?")

USAGE = """usage:
  dimcalc convert VALUE FROM TO
  dimcalc base QUANTITY
  dimcalc dim UNIT
  dimcalc check UNIT1 UNIT2
  dimcalc split QUANTITY U1,U2,...
  dimcalc compare QUANTITY1 QUANTITY2"""

ARITY = {"convert": 3, "base": 1, "dim": 1, "check": 2, "split": 2, "compare": 2}


class _UsageError(Exception):
    pass


def _fmt(x: float) -> str:
    return format(x, ".6g")


def _run(command: str, args: list[str]) -> str:
    if command == "convert":
        if not _DECIMAL_RE.fullmatch(args[0]):
            raise _UsageError(f"VALUE must be a decimal number, got {args[0]!r}")
        return _fmt(Quantity(float(args[0]), args[1]).to(args[2]).value)
    if command == "base":
        return str(parse_quantity(args[0]).in_base())
    if command == "dim":
        return str(parse_unit(args[0]).dimension)
    if command == "check":
        same = parse_unit(args[0]).dimension == parse_unit(args[1]).dimension
        return "compatible" if same else "incompatible"
    if command == "compare":
        first, second = parse_quantity(args[0]), parse_quantity(args[1])
        if first == second:
            return "="
        return "<" if first < second else ">"
    parts = split(parse_quantity(args[0]), [u for u in args[1].split(",")])
    return " ".join(str(p) for p in parts)


def main(argv: list[str]) -> int:
    try:
        if not argv or argv[0] not in ARITY:
            raise _UsageError("unknown or missing command")
        command, args = argv[0], argv[1:]
        if len(args) != ARITY[command]:
            raise _UsageError(f"{command} takes {ARITY[command]} arguments")
        line = _run(command, args)
    except _UsageError as exc:
        print(f"{exc}\n{USAGE}", file=sys.stderr)
        return 2
    except DimcalcError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    print(line, file=sys.stdout)
    return 0
