"""Planning and running queries."""

from dataclasses import dataclass

from .errors import QueryError
from .evaluate import Context, evaluate, is_number
from .parser import parse
from .functions import check_call
from .syntax import Star, SelectItem, Aggregate, Call, Case, Column, walk
from .table import Table, looks_converted


def _format(value) -> str:
    if value is None:
        return ""
    if value is True:
        return "true"
    if value is False:
        return "false"
    text = str(value)
    if any(ch in text for ch in ',"\r\n') or (isinstance(value, str) and looks_converted(text)):
        return '"' + text.replace('"', '""') + '"'
    return text


@dataclass
class Result:
    columns: list[str]
    rows: list[tuple]

    def to_csv(self) -> str:
        lines = [",".join(_format(c) for c in self.columns)]
        lines.extend(",".join(_format(v) for v in row) for row in self.rows)
        return "".join(line + "\n" for line in lines)


def _sort_key(value):
    if value is None:
        return (0, 0)
    if isinstance(value, bool):
        return (1, value)
    if is_number(value):
        return (2, value)
    return (3, value)


class Scope:
    """The columns of the sources joined so far, in row order."""

    def __init__(self, sql: str):
        self.sql = sql  # the query text, for the place of an error
        self.slots: list[tuple[str, str]] = []  # (qualifier, column name)
        self.qualifiers: list[str] = []
        self._cache: dict[Column, int] = {}

    def extended(self, qualifier: str, columns: list[str]) -> "Scope":
        if qualifier in self.qualifiers:
            raise QueryError(f"table name {qualifier!r} is used twice in FROM")
        scope = Scope(self.sql)
        scope.slots = self.slots + [(qualifier, name) for name in columns]
        scope.qualifiers = self.qualifiers + [qualifier]
        return scope

    def index(self, column: Column) -> int:
        if column not in self._cache:
            self._cache[column] = self._resolve(column)
        return self._cache[column]

    def error(self, message: str, node) -> QueryError:
        """A QueryError at the place of node in the query text."""
        return QueryError(message, self.sql, node.pos)

    def _resolve(self, column: Column) -> int:
        if column.qualifier is not None:
            if column.qualifier not in self.qualifiers:
                raise self.error(f"unknown table {column.qualifier!r}", column)
            for i, (q, name) in enumerate(self.slots):
                if q == column.qualifier and name == column.name:
                    return i
            raise self.error(f"unknown column {str(column)!r}", column)
        hits = [(i, q) for i, (q, name) in enumerate(self.slots) if name == column.name]
        if not hits:
            raise self.error(f"unknown column {column.name!r}", column)
        if len({q for _, q in hits}) > 1:
            raise self.error(f"column {column.name!r} is ambiguous", column)
        return hits[0][0]


def _has_aggregate(node) -> bool:
    return any(isinstance(n, Aggregate) for n in walk(node))


def _columns_outside_aggregates(node):
    if isinstance(node, Aggregate):
        return
    if isinstance(node, Column):
        yield node
        return
    for child in _children(node):
        yield from _columns_outside_aggregates(child)


def _children(node):
    if isinstance(node, Call):
        yield from node.args
        return
    if isinstance(node, Case):
        for condition, result in node.branches:
            yield condition
            yield result
        if node.otherwise is not None:
            yield node.otherwise
        return
    for attr in ("operand", "left", "right"):
        child = getattr(node, attr, None)
        if child is not None:
            yield child


def _check_names(node, scope: Scope, where: str):
    for n in walk(node):
        if isinstance(n, Column):
            scope.index(n)
        if isinstance(n, Call):
            try:
                check_call(n.name, len(n.args))
            except QueryError as exc:
                raise scope.error(exc.message, n) from None
        if isinstance(n, Aggregate):
            if where != "SELECT":
                raise QueryError(f"aggregates are not allowed in {where}")
            if n.arg is not None and _has_aggregate(n.arg):
                raise QueryError("aggregates cannot be nested")


def _expand_stars(items, scope: Scope):
    """Replace each "q.*" item by the columns of source q, in order."""
    out = []
    for item in items:
        if isinstance(item.expr, Star):
            if item.expr.qualifier not in scope.qualifiers:
                raise scope.error(f"unknown table {item.expr.qualifier!r}", item.expr)
            out.extend(SelectItem(Column(name, q), None) for q, name in scope.slots if q == item.expr.qualifier)
        else:
            out.append(item)
    return out


def _check(q, scope: Scope) -> bool:
    """Validate names and aggregate use, return whether the query aggregates."""
    stars = q.items is not None and any(isinstance(item.expr, Star) for item in q.items)
    if stars:
        q.items = _expand_stars(q.items, scope)
    exprs = [item.expr for item in q.items or []]
    for e in exprs:
        _check_names(e, scope, "SELECT")
    if q.where is not None:
        _check_names(q.where, scope, "WHERE")
    grouped = {scope.index(c) for c in q.group_by}
    aggregating = bool(q.group_by) or any(_has_aggregate(e) for e in exprs)
    if q.items is None and q.group_by:
        raise QueryError("SELECT * cannot be used with GROUP BY")
    if stars and aggregating:
        raise QueryError("a table's .* cannot be used with aggregation")
    if aggregating:
        for e in exprs:
            for column in _columns_outside_aggregates(e):
                if scope.index(column) not in grouped:
                    raise scope.error(f"column {str(column)!r} must be in GROUP BY or inside an aggregate", column)
    return aggregating


def _table(tables: dict[str, Table], source, sql: str) -> Table:
    if source.table not in tables:
        raise QueryError(f"unknown table {source.table!r}", sql, source.pos)
    return tables[source.table]


def _holds(value, clause: str) -> bool:
    """A WHERE or ON condition keeps a row only when it is TRUE, NULL drops it."""
    if value is not None and not isinstance(value, bool):
        raise QueryError(f"{clause} needs a condition")
    return value is True


def _join(rows: list[tuple], scope: Scope, join, table: Table) -> list[tuple]:
    out = []
    padding = tuple(None for _ in table.columns)
    for left in rows:
        matched = False
        for right in table.rows:
            row = left + right
            if _holds(evaluate(join.on, Context(scope, row)), "ON"):
                out.append(row)
                matched = True
        if join.left and not matched:
            out.append(left + padding)
    return out


def _from(q, tables: dict[str, Table], sql: str) -> tuple[Scope, list[tuple]]:
    """Check the FROM sources and joins, return the scope and the joined rows."""
    first = _table(tables, q.source, sql)
    scope = Scope(sql).extended(q.source.qualifier, first.columns)
    steps = []
    for join in q.joins:
        table = _table(tables, join.source, sql)
        scope = scope.extended(join.source.qualifier, table.columns)
        _check_names(join.on, scope, "ON")
        steps.append((join, table, scope))
    rows = list(first.rows)
    for join, table, step_scope in steps:
        rows = _join(rows, step_scope, join, table)
    return scope, rows


def query(sql: str, tables: dict[str, Table]) -> Result:
    q = parse(sql)
    scope, rows = _from(q, tables, sql)
    aggregating = _check(q, scope)

    if q.where is not None:
        kept = []
        for row in rows:
            if _holds(evaluate(q.where, Context(scope, row)), "WHERE"):
                kept.append(row)
        rows = kept

    if q.items is None:
        names = [name for _, name in scope.slots]
        sources = [(row, list(row)) for row in rows]
    else:
        names = [item.alias or (item.expr.name if isinstance(item.expr, Column) else f"col{i}")
                 for i, item in enumerate(q.items, start=1)]
        if aggregating:
            key_index = [scope.index(c) for c in q.group_by]
            groups: dict[tuple, list[tuple]] = {}
            for row in rows:
                key = tuple(row[i] for i in key_index)
                groups.setdefault(key, []).append(row)
            if not q.group_by and not groups:
                groups[()] = []
            sources = []
            for members in groups.values():
                first = members[0] if members else tuple(None for _ in scope.slots)
                ctx = Context(scope, first, members)
                sources.append((None, [evaluate(item.expr, ctx) for item in q.items]))
        else:
            sources = [(row, [evaluate(item.expr, Context(scope, row)) for item in q.items]) for row in rows]

    for column, descending in reversed(q.order_by):
        if column.qualifier is None and column.name in names:
            idx = names.index(column.name)
            sources.sort(key=lambda s, idx=idx: _sort_key(s[1][idx]), reverse=descending)
        elif not aggregating:
            idx = scope.index(column)
            sources.sort(key=lambda s, idx=idx: _sort_key(s[0][idx]), reverse=descending)
        else:
            raise scope.error(f"cannot order by {str(column)!r}", column)

    out = [tuple(values) for _, values in sources]
    if q.limit is not None:
        out = out[: q.limit]
    return Result(names, out)
