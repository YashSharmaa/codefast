"""Quantities: a value with a unit, conversions and arithmetic."""

import math
import re

from .errors import DimensionError, UnitError
from .parse import parse_unit

_NUMBER_RE = re.compile(r"[+-]?[0-9]+(\.[0-9]+)?([eE][+-]?[0-9]+)?")


class Quantity:
    def __init__(self, value: float, unit: str):
        self._unit = parse_unit(unit)
        self.value = value
        self.unit = unit.strip()

    @property
    def dimension(self):
        return self._unit.dimension

    def _require_linear(self, other=None) -> None:
        for q in (self, other):
            if isinstance(q, Quantity) and q._unit.affine:
                raise DimensionError(f"no arithmetic on {q.unit} quantities")

    def to(self, unit: str) -> "Quantity":
        target = parse_unit(unit)
        if target.dimension != self.dimension:
            raise DimensionError(f"cannot convert {self.unit} ({self.dimension}) to {target.name} ({target.dimension})")
        base = self.value * self._unit.factor + self._unit.offset
        return Quantity((base - target.offset) / target.factor, unit)

    def in_base(self) -> "Quantity":
        return Quantity(self.value * self._unit.factor + self._unit.offset, str(self.dimension))

    def _same_dimension(self, other: "Quantity") -> None:
        self._require_linear(other)
        if other.dimension != self.dimension:
            raise DimensionError(f"cannot combine {self.unit} with {other.unit}")

    def __add__(self, other):
        if not isinstance(other, Quantity):
            return NotImplemented
        self._same_dimension(other)
        return Quantity(self.value + other.to(self.unit).value, self.unit)

    def __sub__(self, other):
        if not isinstance(other, Quantity):
            return NotImplemented
        self._same_dimension(other)
        return Quantity(self.value - other.to(self.unit).value, self.unit)

    def __mul__(self, other):
        self._require_linear(other)
        if isinstance(other, Quantity):
            return Quantity(self.value * other.value, f"({self.unit})*({other.unit})")
        if isinstance(other, (int, float)):
            return Quantity(self.value * other, self.unit)
        return NotImplemented

    def __rmul__(self, other):
        return self.__mul__(other)

    def __truediv__(self, other):
        self._require_linear(other)
        if isinstance(other, Quantity):
            return Quantity(self.value / other.value, f"({self.unit})/({other.unit})")
        if isinstance(other, (int, float)):
            return Quantity(self.value / other, self.unit)
        return NotImplemented

    def __rtruediv__(self, other):
        self._require_linear()
        if isinstance(other, (int, float)):
            return Quantity(other / self.value, f"1/({self.unit})")
        return NotImplemented

    def _base_values(self, other) -> tuple[float, float]:
        if not isinstance(other, Quantity):
            raise TypeError("quantities compare only with quantities")
        if other.dimension != self.dimension:
            raise DimensionError(f"cannot compare {self.unit} with {other.unit}")
        mine = self.value * self._unit.factor + self._unit.offset
        theirs = other.value * other._unit.factor + other._unit.offset
        return mine, theirs

    def __eq__(self, other):
        if not isinstance(other, Quantity):
            return NotImplemented
        a, b = self._base_values(other)
        return math.isclose(a, b, rel_tol=1e-9, abs_tol=1e-12)

    __hash__ = None

    def __lt__(self, other):
        a, b = self._base_values(other)
        return a < b and not self == other

    def __le__(self, other):
        a, b = self._base_values(other)
        return a < b or self == other

    def __gt__(self, other):
        a, b = self._base_values(other)
        return a > b and not self == other

    def __ge__(self, other):
        a, b = self._base_values(other)
        return a > b or self == other

    def __str__(self) -> str:
        return f"{format(self.value, '.6g')} {self.unit}"

    def __repr__(self) -> str:
        return f"Quantity({self.value!r}, {self.unit!r})"


def parse_quantity(text: str) -> Quantity:
    stripped = text.strip()
    m = _NUMBER_RE.match(stripped)
    if m is None:
        raise UnitError(f"{text!r} does not start with a number")
    rest = stripped[m.end():].strip()
    return Quantity(float(m.group(0)), rest or "1")


def convert(value: float, from_unit: str, to_unit: str) -> float:
    return Quantity(value, from_unit).to(to_unit).value


def split(q: Quantity, units: list[str]) -> list[Quantity]:
    if not units:
        raise UnitError("split needs at least one unit")
    q._require_linear()
    targets = [parse_unit(u) for u in units]
    for target in targets:
        if target.affine:
            raise DimensionError(f"cannot split into {target.name}")
        if target.dimension != q.dimension:
            raise DimensionError(f"cannot split {q.unit} into {target.name}")
    sign = -1 if q.value < 0 else 1
    remaining = abs(q.value) * q._unit.factor
    parts = []
    for i, target in enumerate(targets):
        x = remaining / target.factor
        if i < len(targets) - 1:
            x = math.floor(x + 1e-9)
            remaining -= x * target.factor
        parts.append(Quantity(sign * x, target.name))
    return parts
