"""Syntax tree of a query."""

from dataclasses import dataclass, field


@dataclass(frozen=True)
class Literal:
    value: object


@dataclass(frozen=True)
class Column:
    name: str
    qualifier: str | None = None  # table name or alias in "q.name"
    pos: int | None = field(default=None, compare=False)  # where the name starts in the query

    def __str__(self):
        return self.name if self.qualifier is None else f"{self.qualifier}.{self.name}"


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
class IsTruth:
    operand: object
    value: bool  # IS TRUE or IS FALSE
    negated: bool


@dataclass(frozen=True)
class Case:
    branches: tuple  # ((condition, result), ...)
    otherwise: object  # None when there is no ELSE


@dataclass(frozen=True)
class Aggregate:
    func: str
    arg: object  # None for COUNT(*)


@dataclass(frozen=True)
class Call:
    name: str  # upper case
    args: tuple
    pos: int | None = field(default=None, compare=False)


@dataclass(frozen=True)
class Star:
    qualifier: str  # "q.*" in a select list
    pos: int | None = field(default=None, compare=False)


@dataclass(frozen=True)
class SelectItem:
    expr: object
    alias: str | None


@dataclass(frozen=True)
class Source:
    table: str
    alias: str | None = None
    pos: int | None = field(default=None, compare=False)

    @property
    def qualifier(self) -> str:
        return self.alias if self.alias is not None else self.table


@dataclass(frozen=True)
class Join:
    left: bool  # LEFT JOIN keeps unmatched rows
    source: Source
    on: object


@dataclass
class Query:
    items: list[SelectItem] | None  # None means SELECT *
    source: Source
    joins: list[Join] = field(default_factory=list)
    where: object = None
    group_by: list[Column] = field(default_factory=list)
    order_by: list[tuple[Column, bool]] = field(default_factory=list)  # (column, descending)
    limit: int | None = None


def walk(node):
    """Yield node and every node below it."""
    yield node
    if isinstance(node, Unary):
        yield from walk(node.operand)
    elif isinstance(node, Binary):
        yield from walk(node.left)
        yield from walk(node.right)
    elif isinstance(node, (IsNull, IsTruth)):
        yield from walk(node.operand)
    elif isinstance(node, Aggregate) and node.arg is not None:
        yield from walk(node.arg)
    elif isinstance(node, Case):
        for condition, result in node.branches:
            yield from walk(condition)
            yield from walk(result)
        if node.otherwise is not None:
            yield from walk(node.otherwise)
    elif isinstance(node, Call):
        for arg in node.args:
            yield from walk(arg)
