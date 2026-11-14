"""Parsing unit expressions such as kg*m/s^2 or J/(kg*K)."""

import re

from .dimension import DIMENSIONLESS
from .errors import UnitError
from .units import Unit, lookup

_TOKEN_RE = re.compile(r"\s*(?:([A-Za-z]+)|([0-9]+)|(\S))")


def _tokens(text: str) -> list[tuple[str, str]]:
    tokens = []
    pos = 0
    while pos < len(text):
        m = _TOKEN_RE.match(text, pos)
        if m is None:  # only trailing whitespace is left
            break
        if m.group(1):
            tokens.append(("name", m.group(1)))
        elif m.group(2):
            tokens.append(("int", m.group(2)))
        else:
            if m.group(3) not in "*/^()-":
                raise UnitError(f"unexpected character {m.group(3)!r} in unit {text!r}")
            tokens.append(("op", m.group(3)))
        pos = m.end()
    return tokens


class _Parser:
    def __init__(self, text: str):
        self.text = text
        self.tokens = _tokens(text)
        self.i = 0

    def peek(self):
        return self.tokens[self.i] if self.i < len(self.tokens) else (None, None)

    def take(self):
        tok = self.peek()
        if tok[0] is None:
            raise UnitError(f"unit {self.text!r} ends too early")
        self.i += 1
        return tok

    def expr(self) -> tuple[float, object]:
        factor, dim = self.power()
        while self.peek() in (("op", "*"), ("op", "/")):
            op = self.take()[1]
            f2, d2 = self.power()
            if op == "*":
                factor, dim = factor * f2, dim * d2
            else:
                factor, dim = factor / f2, dim / d2
        return factor, dim

    def power(self) -> tuple[float, object]:
        factor, dim = self.atom()
        if self.peek() == ("op", "^"):
            self.take()
            sign = 1
            if self.peek() == ("op", "-"):
                self.take()
                sign = -1
            kind, value = self.take()
            if kind != "int":
                raise UnitError(f"exponent must be an integer in {self.text!r}")
            n = sign * int(value)
            factor, dim = factor ** n, dim ** n
        return factor, dim

    def atom(self) -> tuple[float, object]:
        kind, value = self.take()
        if kind == "name":
            unit = lookup(value)
            if unit.affine:
                raise UnitError(f"{value} cannot be part of a larger unit expression")
            return unit.factor, unit.dimension
        if kind == "int" and value == "1":
            return 1.0, DIMENSIONLESS
        if (kind, value) == ("op", "("):
            result = self.expr()
            if self.take() != ("op", ")"):
                raise UnitError(f"missing ')' in {self.text!r}")
            return result
        raise UnitError(f"unexpected {value!r} in unit {self.text!r}")


def parse_unit(text: str) -> Unit:
    name = text.strip()
    if not name:
        raise UnitError("empty unit")
    if name.isalpha():
        whole = lookup(name)
        if whole.affine:
            return whole
    parser = _Parser(name)
    factor, dim = parser.expr()
    if parser.i != len(parser.tokens):
        raise UnitError(f"unexpected {parser.tokens[parser.i][1]!r} in unit {name!r}")
    return Unit(name, factor, dim)
