"""Dimension vectors over the six base dimensions."""

from dataclasses import dataclass

BASES = ("m", "kg", "s", "A", "K", "mol")


@dataclass(frozen=True)
class Dimension:
    exponents: tuple[int, int, int, int, int, int]

    def __post_init__(self):
        exps = tuple(int(e) for e in self.exponents)
        if len(exps) != len(BASES):
            raise ValueError("a dimension has six exponents")
        object.__setattr__(self, "exponents", exps)

    def __mul__(self, other: "Dimension") -> "Dimension":
        return Dimension(tuple(a + b for a, b in zip(self.exponents, other.exponents)))

    def __truediv__(self, other: "Dimension") -> "Dimension":
        return Dimension(tuple(a - b for a, b in zip(self.exponents, other.exponents)))

    def __pow__(self, n: int) -> "Dimension":
        return Dimension(tuple(a * n for a in self.exponents))

    def is_dimensionless(self) -> bool:
        return not any(self.exponents)

    def __str__(self) -> str:
        def term(base: str, exp: int) -> str:
            return base if exp == 1 else f"{base}^{exp}"

        positive = [term(b, e) for b, e in zip(BASES, self.exponents) if e > 0]
        negative = [term(b, -e) for b, e in zip(BASES, self.exponents) if e < 0]
        text = "*".join(positive) if positive else "1"
        if negative:
            text += "/" + "*".join(negative)
        return text


DIMENSIONLESS = Dimension((0, 0, 0, 0, 0, 0))


def base(index: int) -> Dimension:
    exps = [0] * len(BASES)
    exps[index] = 1
    return Dimension(tuple(exps))
