"""Scalar functions callable from queries, such as UPPER(name) or ROUND(score, 1)."""

from decimal import ROUND_HALF_UP, Decimal

from .errors import QueryError


def _is_number(v) -> bool:
    return isinstance(v, (int, float)) and not isinstance(v, bool)


def _string(name: str, v) -> str:
    if not isinstance(v, str):
        raise QueryError(f"{name} needs a string, got {v!r}")
    return v


def _number(name: str, v):
    if not _is_number(v):
        raise QueryError(f"{name} needs a number, got {v!r}")
    return v


def _integer(name: str, v, least: int) -> int:
    if not isinstance(v, int) or isinstance(v, bool) or v < least:
        raise QueryError(f"{name} needs an integer of at least {least}, got {v!r}")
    return v


def _upper(s):
    return _string("UPPER", s).upper()


def _lower(s):
    return _string("LOWER", s).lower()


def _length(s):
    return len(_string("LENGTH", s))


def _trim(s):
    return _string("TRIM", s).strip()


def _substr(s, start, length=None):
    s = _string("SUBSTR", s)
    begin = _integer("SUBSTR", start, 1) - 1
    if length is None:
        return s[begin:]
    return s[begin:begin + _integer("SUBSTR", length, 0)]


def _replace(s, old, new):
    s, old, new = _string("REPLACE", s), _string("REPLACE", old), _string("REPLACE", new)
    if old == "":
        return s
    return s.replace(old, new)


def _instr(s, sub):
    return _string("INSTR", s).find(_string("INSTR", sub)) + 1


def _text(v) -> str:
    if v is True:
        return "true"
    if v is False:
        return "false"
    return str(v)


def _concat(*args):
    return "".join(_text(v) for v in args if v is not None)


def _abs(n):
    return abs(_number("ABS", n))


def _round(n, digits=None):
    n = _number("ROUND", n)
    if digits is None:
        return int(Decimal(n).quantize(Decimal(1), rounding=ROUND_HALF_UP))
    digits = _integer("ROUND", digits, 0)
    step = Decimal(1).scaleb(-digits)
    return float(Decimal(n).quantize(step, rounding=ROUND_HALF_UP))


def _coalesce(*args):
    for v in args:
        if v is not None:
            return v
    return None


def _nullif(a, b):
    from .evaluate import compare

    return None if compare("=", a, b) is True else a


# name: (implementation, least arguments, most arguments or None, passes NULL through)
_FUNCTIONS = {
    "UPPER": (_upper, 1, 1, True),
    "LOWER": (_lower, 1, 1, True),
    "LENGTH": (_length, 1, 1, True),
    "TRIM": (_trim, 1, 1, True),
    "SUBSTR": (_substr, 2, 3, True),
    "REPLACE": (_replace, 3, 3, True),
    "INSTR": (_instr, 2, 2, True),
    "CONCAT": (_concat, 1, None, False),
    "ABS": (_abs, 1, 1, True),
    "ROUND": (_round, 1, 2, True),
    "COALESCE": (_coalesce, 1, None, False),
    "NULLIF": (_nullif, 2, 2, False),
}


def check_call(name: str, count: int) -> None:
    """Raise QueryError unless NAME is a function that takes COUNT arguments."""
    entry = _FUNCTIONS.get(name.upper())
    if entry is None:
        raise QueryError(f"unknown function {name!r}")
    _, least, most, _ = entry
    if count < least or (most is not None and count > most):
        if most is None:
            want = f"at least {least}"
        elif least == most:
            want = str(least)
        else:
            want = f"{least} to {most}"
        raise QueryError(f"{name.upper()} takes {want} arguments, got {count}")


def call_function(name: str, args: list):
    """Call the function NAME (any case) with the given argument values."""
    check_call(name, len(args))
    func, _, _, null_in_null_out = _FUNCTIONS[name.upper()]
    if null_in_null_out and any(v is None for v in args):
        return None
    return func(*args)
