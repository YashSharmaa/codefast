"""Evaluating expressions on rows and groups."""

from .errors import QueryError
from .functions import call_function
from .syntax import Aggregate, Binary, Call, Case, Column, IsNull, IsTruth, Literal, Unary


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


def compare(op: str, a, b) -> bool | None:
    """Compare two values, NULL when either is NULL."""
    if a is None or b is None:
        return None
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


def _truth(v, op: str) -> bool | None:
    if v is not None and not isinstance(v, bool):
        raise QueryError(f"{op} needs true, false or NULL, got {v!r}")
    return v


def logic(op: str, a, b) -> bool | None:
    """Three-valued AND and OR."""
    a, b = _truth(a, op), _truth(b, op)
    if op == "AND":
        if a is False or b is False:
            return False
        return None if a is None or b is None else True
    if a is True or b is True:
        return True
    return None if a is None or b is None else False


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
    """Supplies column values and, for groups, the rows of the group.

    scope maps a Column node to its position in the row (see engine.Scope).
    """

    def __init__(self, scope, row: tuple, group: list[tuple] | None = None):
        self.scope = scope
        self.row = row
        self.group = group

    def value(self, column):
        return self.row[self.scope.index(column)]


def evaluate(node, ctx: Context):
    if isinstance(node, Literal):
        return node.value
    if isinstance(node, Column):
        return ctx.value(node)
    if isinstance(node, Unary):
        v = evaluate(node.operand, ctx)
        if node.op == "NOT":
            v = _truth(v, "NOT")
            return None if v is None else not v
        if v is None:
            return None
        if not is_number(v):
            raise QueryError(f"unary minus needs a number, got {v!r}")
        return -v
    if isinstance(node, IsNull):
        v = evaluate(node.operand, ctx)
        return (v is not None) if node.negated else (v is None)
    if isinstance(node, IsTruth):
        v = _truth(evaluate(node.operand, ctx), "IS")
        return (v is not node.value) if node.negated else (v is node.value)
    if isinstance(node, Binary):
        if node.op in ("AND", "OR"):
            return logic(node.op, evaluate(node.left, ctx), evaluate(node.right, ctx))
        a, b = evaluate(node.left, ctx), evaluate(node.right, ctx)
        if node.op in ("+", "-", "*", "/"):
            return _arith(node.op, a, b)
        return compare(node.op, a, b)
    if isinstance(node, Aggregate):
        rows = ctx.group or []
        if node.arg is None:
            return aggregate("COUNT", rows, count_star=True)
        values = [evaluate(node.arg, Context(ctx.scope, r)) for r in rows]
        return aggregate(node.func, values)
    if isinstance(node, Case):
        for condition, result in node.branches:
            if _truth(evaluate(condition, ctx), "WHEN") is True:
                return evaluate(result, ctx)
        return None if node.otherwise is None else evaluate(node.otherwise, ctx)
    if isinstance(node, Call):
        return call_function(node.name, [evaluate(arg, ctx) for arg in node.args])
    raise TypeError(f"unknown node {node!r}")
