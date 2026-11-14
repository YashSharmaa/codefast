"""Evaluating expressions on rows and groups."""

from .errors import QueryError
from .syntax import Aggregate, Binary, Column, IsNull, Literal, Unary


def is_number(v) -> bool:
    return isinstance(v, (int, float)) and not isinstance(v, bool)


def _arith(op: str, a, b):
    for v in (a, b):
        if v is not None and not is_number(v):
            raise QueryError(f"arithmetic {op} needs numbers, got {v!r}")
    if a is None or b is None:
        return None
    if op == "+":
        return a + b
    if op == "-":
        return a - b
    if op == "*":
        return a * b
    if b == 0:
        return None
    return a / b


def compare(op: str, a, b) -> bool:
    if a is None or b is None:
        return False
    if is_number(a) and is_number(b):
        pass
    elif isinstance(a, str) and isinstance(b, str):
        pass
    elif isinstance(a, bool) and isinstance(b, bool) and op in ("=", "!="):
        pass
    else:
        raise QueryError(f"cannot compare {a!r} with {b!r}")
    if op == "=":
        return a == b
    if op == "!=":
        return a != b
    if op == "<":
        return a < b
    if op == "<=":
        return a <= b
    if op == ">":
        return a > b
    return a >= b


def _boolean(v, op: str) -> bool:
    if not isinstance(v, bool):
        raise QueryError(f"{op} needs true or false, got {v!r}")
    return v


def aggregate(func: str, values: list, count_star: bool = False):
    if count_star:
        return len(values)
    present = [v for v in values if v is not None]
    if func == "COUNT":
        return len(present)
    if not present:
        return None
    if func in ("SUM", "AVG"):
        if not all(is_number(v) for v in present):
            raise QueryError(f"{func} needs numbers")
        total = sum(present)
        return total if func == "SUM" else total / len(present)
    if all(is_number(v) for v in present) or all(isinstance(v, str) for v in present):
        return min(present) if func == "MIN" else max(present)
    raise QueryError(f"{func} cannot mix numbers and strings")


class Context:
    """Supplies column values and, for groups, the rows of the group."""

    def __init__(self, columns: list[str], row: tuple, group: list[tuple] | None = None):
        self.columns = columns
        self.row = row
        self.group = group

    def value(self, name: str):
        return self.row[self.columns.index(name)]


def evaluate(node, ctx: Context):
    if isinstance(node, Literal):
        return node.value
    if isinstance(node, Column):
        return ctx.value(node.name)
    if isinstance(node, Unary):
        v = evaluate(node.operand, ctx)
        if node.op == "NOT":
            return not _boolean(v, "NOT")
        if v is None:
            return None
        if not is_number(v):
            raise QueryError(f"unary minus needs a number, got {v!r}")
        return -v
    if isinstance(node, IsNull):
        v = evaluate(node.operand, ctx)
        return (v is not None) if node.negated else (v is None)
    if isinstance(node, Binary):
        if node.op in ("AND", "OR"):
            a = _boolean(evaluate(node.left, ctx), node.op)
            b = _boolean(evaluate(node.right, ctx), node.op)
            return (a and b) if node.op == "AND" else (a or b)
        a, b = evaluate(node.left, ctx), evaluate(node.right, ctx)
        if node.op in ("+", "-", "*", "/"):
            return _arith(node.op, a, b)
        return compare(node.op, a, b)
    if isinstance(node, Aggregate):
        rows = ctx.group or []
        if node.arg is None:
            return aggregate("COUNT", rows, count_star=True)
        values = [evaluate(node.arg, Context(ctx.columns, r)) for r in rows]
        return aggregate(node.func, values)
    raise TypeError(f"unknown node {node!r}")
