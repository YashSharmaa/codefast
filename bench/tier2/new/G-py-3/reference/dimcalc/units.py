"""The unit table and name lookup with SI prefixes."""

from dataclasses import dataclass

from .dimension import DIMENSIONLESS, Dimension, base
from .errors import UnitError

M, KG, S, A, K, MOL = (base(i) for i in range(6))


@dataclass(frozen=True)
class Unit:
    name: str
    factor: float
    dimension: Dimension
    offset: float = 0.0

    @property
    def affine(self) -> bool:
        return self.offset != 0.0

PREFIXES = {"G": 1e9, "M": 1e6, "k": 1e3, "c": 1e-2, "m": 1e-3, "u": 1e-6, "n": 1e-9}

FORCE = KG * M / S ** 2
ENERGY = FORCE * M
POWER = ENERGY / S
PRESSURE = FORCE / M ** 2

# name: (factor, dimension, offset, prefixable)
TABLE = {
    "m": (1.0, M, 0.0, True),
    "s": (1.0, S, 0.0, True),
    "A": (1.0, A, 0.0, True),
    "K": (1.0, K, 0.0, True),
    "mol": (1.0, MOL, 0.0, True),
    "g": (0.001, KG, 0.0, True),
    "N": (1.0, FORCE, 0.0, True),
    "J": (1.0, ENERGY, 0.0, True),
    "W": (1.0, POWER, 0.0, True),
    "Pa": (1.0, PRESSURE, 0.0, True),
    "bar": (100000.0, PRESSURE, 0.0, True),
    "Hz": (1.0, DIMENSIONLESS / S, 0.0, True),
    "C": (1.0, S * A, 0.0, True),
    "V": (1.0, POWER / A, 0.0, True),
    "L": (0.001, M ** 3, 0.0, True),
    "min": (60.0, S, 0.0, False),
    "h": (3600.0, S, 0.0, False),
    "day": (86400.0, S, 0.0, False),
    "in": (0.0254, M, 0.0, False),
    "ft": (0.3048, M, 0.0, False),
    "yd": (0.9144, M, 0.0, False),
    "mi": (1609.344, M, 0.0, False),
    "lb": (0.45359237, KG, 0.0, False),
    "oz": (0.028349523125, KG, 0.0, False),
    "degC": (1.0, K, 273.15, False),
    "degF": (5 / 9, K, 459.67 * 5 / 9, False),
}


def lookup(name: str) -> Unit:
    entry = TABLE.get(name)
    if entry is not None:
        factor, dim, offset, _ = entry
        return Unit(name, factor, dim, offset)
    if len(name) > 1 and name[0] in PREFIXES:
        entry = TABLE.get(name[1:])
        if entry is not None and entry[3]:
            factor, dim, offset, _ = entry
            return Unit(name, PREFIXES[name[0]] * factor, dim, offset)
    raise UnitError(f"unknown unit {name!r}")
