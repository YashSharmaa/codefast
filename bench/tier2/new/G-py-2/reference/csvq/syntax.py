"""Syntax tree of a query."""

from dataclasses import dataclass, field


@dataclass(frozen=True)
class Literal:
    value: object


@dataclass(frozen=True)
class Column:
    name: str


@dataclass(frozen=True)
class Unary:
    op: str  # "-" or "NOT"
    operand: object


@dataclass(frozen=True)
class Binary:
    op: str
    left: object
    right: object


@dataclass(frozen=True)
class IsNull:
    operand: object
    negated: bool


@dataclass(frozen=True)
class Aggregate:
    func: str
    arg: object  # None for COUNT(*)


@dataclass(frozen=True)
class SelectItem:
    expr: object
    alias: str | None


@dataclass
class Query:
    items: list[SelectItem] | None  # None means SELECT *
    table: str
    where: object = None
    group_by: list[str] = field(default_factory=list)
    order_by: list[tuple[str, bool]] = field(default_factory=list)  # (name, descending)
    limit: int | None = None


def walk(node):
    """Yield node and every node below it."""
    yield node
    if isinstance(node, Unary):
        yield from walk(node.operand)
    elif isinstance(node, Binary):
        yield from walk(node.left)
        yield from walk(node.right)
    elif isinstance(node, IsNull):
        yield from walk(node.operand)
    elif isinstance(node, Aggregate) and node.arg is not None:
        yield from walk(node.arg)
