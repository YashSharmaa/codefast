# dimcalc: unit conversion with dimensional analysis

`dimcalc` converts physical quantities between units, checks that units are compatible, and
does arithmetic on quantities while tracking their dimensions. It is a Python package with a
command line. The project will be checked by tests written against the interface below, so
names, signatures, error types, unit tables and output formats must match exactly.

## Package layout

| Module | Public names |
|---|---|
| `dimcalc/errors.py` | `DimcalcError(Exception)`, `UnitError(DimcalcError)`, `DimensionError(DimcalcError)` |
| `dimcalc/dimension.py` | `Dimension`, `DIMENSIONLESS` |
| `dimcalc/units.py` | `Unit`, `lookup(name: str) -> Unit` |
| `dimcalc/parse.py` | `parse_unit(text: str) -> Unit` |
| `dimcalc/quantity.py` | `Quantity`, `parse_quantity(text: str) -> Quantity`, `convert(value: float, from_unit: str, to_unit: str) -> float`, `split(q: Quantity, units: list[str]) -> list[Quantity]` |
| `dimcalc/cli.py` | `main(argv: list[str]) -> int` |
| `dimcalc/__main__.py` | `python -m dimcalc ...` runs `main(sys.argv[1:])` and exits with its result |

`dimcalc/__init__.py` re-exports every name in the table except `main`.

`UnitError` means a unit text cannot be understood. `DimensionError` means the operation is not allowed for these dimensions.

## Dimensions

There are six base dimensions, always in this order: `m`, `kg`, `s`, `A`, `K`, `mol`.
`Dimension(exponents: tuple[int, int, int, int, int, int])` is immutable, hashable and compares
by its exponents. `d1 * d2` and `d1 / d2` add and subtract exponents, `d ** n` multiplies them
by the integer n. `DIMENSIONLESS` has all exponents 0.

`str(d)`: the bases with a positive exponent in the order above joined by `*`, then, if any
exponent is negative, `/` followed by those bases with the absolute exponent joined by `*`. An
exponent of 1 is not written, others are written as `^n`. With no positive exponent the text
starts with `1`, and the dimensionless text is `1`. Examples: `m*kg/s^2`, `1/s`, `m^2/s^2*K`.
In this text every base after the `/` is in the denominator, so it is not read like a unit
expression. The unit table below writes dimensions as unit expressions.

## Units

`Unit` is a frozen dataclass with fields `name: str`, `factor: float`, `dimension: Dimension`
and `offset: float = 0.0`. A value v in the unit equals `v * factor + offset` in base units.

| Name | Factor and dimension | Prefixes |
|---|---|---|
| `m`, `s`, `A`, `K`, `mol` | 1, the base | yes |
| `g` | 0.001 kg | yes |
| `N` | 1 m*kg/s^2 | yes |
| `J` | 1 m^2*kg/s^2 | yes |
| `W` | 1 m^2*kg/s^3 | yes |
| `Pa` | 1 kg/(m*s^2) | yes |
| `bar` | 100000 kg/(m*s^2) | yes |
| `Hz` | 1 1/s | yes |
| `C` | 1 s*A | yes |
| `V` | 1 m^2*kg/(s^3*A) | yes |
| `L` | 0.001 m^3 | yes |
| `min`, `h`, `day` | 60, 3600, 86400 s | no |
| `in`, `ft`, `yd`, `mi` | 0.0254, 0.3048, 0.9144, 1609.344 m | no |
| `lb`, `oz` | 0.45359237, 0.028349523125 kg | no |
| `degC` | factor 1, offset 273.15, dimension K | no |
| `degF` | factor 5/9, offset 459.67 * 5/9, dimension K | no |

Prefixes: `G` 1e9, `M` 1e6, `k` 1e3, `c` 1e-2, `m` 1e-3, `u` 1e-6, `n` 1e-9. `lookup(name)`
first looks for the exact name in the table (so `min` is minutes and `m` is metres). Otherwise,
if the name is a prefix followed by a unit marked yes, the result has the factor multiplied by
the prefix (so `kg` has factor 1 and `mm` 0.001). Names are case-sensitive. Anything else raises
`UnitError`. The returned `Unit` has `name` equal to the name looked up.

## Unit expressions

`parse_unit` reads expressions such as `km/h`, `kg*m/s^2`, `J/(kg*K)`, `1/s`, `m^-1`:

- an atom is a unit name (ASCII letters), `1`, or a parenthesised expression
- an atom may be followed by `^` and an integer with an optional `-` (`m^2`, `s^-1`, `m^0`)
- atoms are joined by `*` and `/`, which group left to right, so `J/kg/K` means `J/(kg*K)`
- spaces are allowed around tokens

The result is a `Unit` whose `name` is the input text with leading and trailing spaces
removed, whose factor and dimension are combined from the parts, and whose offset is 0.
`degC` and `degF` may only be the whole expression (then their own offset is kept). Inside a
larger expression they raise `UnitError`. Empty text, unknown names, a missing operand, a
missing or non-integer exponent and unbalanced parentheses raise `UnitError`.

## Quantities

`Quantity(value: float, unit: str)` parses the unit (raising `UnitError`) and exposes `value`,
`unit` (the text with surrounding spaces removed) and `dimension`.

- `q.to(unit)` returns the quantity in another unit: `UnitError` if the unit does not parse,
  `DimensionError` if the dimensions differ. The new value is
  `((value * f1 + o1) - o2) / f2` and the new `unit` is the target text.
- `q.in_base()` returns the quantity in base units with `unit` equal to `str(q.dimension)`.
- `q1 + q2` and `q1 - q2`: the dimensions must be equal, the result has the unit of `q1`.
- `q1 * q2` gives unit `(u1)*(u2)` and `q1 / q2` gives `(u1)/(u2)`, values multiplied or divided.
- `q * x`, `x * q` and `q / x` with an `int` or `float` x keep the unit of q, `x / q` gives unit
  `1/(u)`.
- Any of these operators on a quantity whose unit is `degC` or `degF` raises `DimensionError`,
  as does `+` or `-` with mismatched dimensions.
- `str(q)` is `format(value, ".6g")`, a space, then the unit text.
- `==`, `!=`, `<`, `<=`, `>` and `>=` compare two quantities of the same dimension by their
  values in base units (offsets included, so `degC` and `degF` compare correctly). Two values
  are equal when `math.isclose(a, b, rel_tol=1e-9, abs_tol=1e-12)` holds, and `<` and `>` are
  false for equal values. Ordering quantities of different dimensions raises `DimensionError`,
  and `==` between them does too. Quantities are not hashable.

`parse_quantity(text)` reads a number (`[+-]?[0-9]+(\.[0-9]+)?([eE][+-]?[0-9]+)?`) at the start
of the text, then optional spaces, then a unit expression. With nothing after the number the
unit is `1`. A text not starting with such a number raises `UnitError`.

`convert(value, from_unit, to_unit)` returns `Quantity(value, from_unit).to(to_unit).value`.
`split(q, units)` expresses q as a sum over the listed units, largest first as listed, for
example 70 in as 5 ft and 10 in. Every unit must have q's dimension (`DimensionError`) and none
may be `degC` or `degF` (`DimensionError`), and q itself may not be in `degC` or `degF`
(`DimensionError`). An empty list raises `UnitError`. Each part but the
last has an integer value: `floor(x + 1e-9)` where x is what remains, expressed in that unit.
The last part takes the remainder as a float. A negative q is split as its absolute value and
then every part is negated.

## Command line

`main(argv)` writes to `sys.stdout` and `sys.stderr` as they are at call time and returns the
exit status without calling `sys.exit`. Numbers are printed with `format(x, ".6g")`.

```
convert VALUE FROM TO     the converted number, for example: convert 100 degC degF -> 212
base QUANTITY             str(parse_quantity(QUANTITY).in_base()), for example: base "3 kN" -> 3000 m*kg/s^2
dim UNIT                  str of the unit's dimension
check UNIT1 UNIT2         compatible or incompatible (same dimension or not), exit status 0 either way
split QUANTITY U1,U2,...  the parts, each as "<number> <unit>", joined by single spaces
compare Q1 Q2             <, = or >, comparing parse_quantity(Q1) with parse_quantity(Q2)
```

Each command prints one line. A `DimcalcError` prints `error: <message>` as one line to stderr
and returns 1. A usage error (no command, an unknown command, a wrong number of arguments, a
VALUE that is not a decimal number) prints a message to stderr and returns 2.

## Constraints

Python 3.15, standard library only. Put your own tests anywhere except `tests_hidden/`, which
is reserved for the checking tests and will be replaced.
