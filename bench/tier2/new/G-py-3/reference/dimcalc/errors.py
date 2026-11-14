"""Error types for dimcalc."""


class DimcalcError(Exception):
    """Base class of every dimcalc error."""


class UnitError(DimcalcError):
    """A unit text cannot be understood."""


class DimensionError(DimcalcError):
    """The operation is not allowed for these dimensions."""
