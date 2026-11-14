"""Planning and running queries."""

from dataclasses import dataclass

from .errors import QueryError
from .evaluate import Context, evaluate, is_number
from .parser import parse
from .syntax import Aggregate, Column, walk
from .table import Table


def _format(value) -> str:
    if value is None:
        return ""
    if value is True:
        return "true"
    if value is False:
        return "false"
    text = str(value)
    if any(ch in text for ch in ',"\r\n'):
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


def _has_aggregate(node) -> bool:
    return any(isinstance(n, Aggregate) for n in walk(node))


def _columns_outside_aggregates(node):
    if isinstance(node, Aggregate):
        return
    if isinstance(node, Column):
        yield node.name
        return
    for child in _children(node):
        yield from _columns_outside_aggregates(child)


def _children(node):
    for attr in ("operand", "left", "right"):
        child = getattr(node, attr, None)
        if child is not None:
            yield child


def _check(q, table: Table) -> bool:
    """Validate names and aggregate use, return whether the query aggregates."""
    exprs = [item.expr for item in q.items or []]
    for node in exprs + ([q.where] if q.where is not None else []):
        for n in walk(node):
            if isinstance(n, Column):
                table.index_of(n.name)
            if isinstance(n, Aggregate) and n.arg is not None and _has_aggregate(n.arg):
                raise QueryError("aggregates cannot be nested")
    for name in q.group_by:
        table.index_of(name)
    if q.where is not None and _has_aggregate(q.where):
        raise QueryError("aggregates are not allowed in WHERE")
    aggregating = bool(q.group_by) or any(_has_aggregate(e) for e in exprs)
    if q.items is None and q.group_by:
        raise QueryError("SELECT * cannot be used with GROUP BY")
    if aggregating:
        for e in exprs:
            for name in _columns_outside_aggregates(e):
                if name not in q.group_by:
                    raise QueryError(f"column {name!r} must be in GROUP BY or inside an aggregate")
    return aggregating


def query(sql: str, tables: dict[str, Table]) -> Result:
    q = parse(sql)
    if q.table not in tables:
        raise QueryError(f"unknown table {q.table!r}")
    table = tables[q.table]
    aggregating = _check(q, table)
    cols = table.columns

    rows = table.rows
    if q.where is not None:
        kept = []
        for row in rows:
            v = evaluate(q.where, Context(cols, row))
            if not isinstance(v, bool):
                raise QueryError("WHERE needs a condition")
            if v:
                kept.append(row)
        rows = kept

    if q.items is None:
        names = list(cols)
        sources = [(row, list(row)) for row in rows]
    else:
        names = [item.alias or (item.expr.name if isinstance(item.expr, Column) else f"col{i}")
                 for i, item in enumerate(q.items, start=1)]
        if aggregating:
            groups: dict[tuple, list[tuple]] = {}
            for row in rows:
                key = tuple(row[cols.index(g)] for g in q.group_by)
                groups.setdefault(key, []).append(row)
            if not q.group_by and not groups:
                groups[()] = []
            sources = []
            for members in groups.values():
                first = members[0] if members else tuple(None for _ in cols)
                ctx = Context(cols, first, members)
                sources.append((None, [evaluate(item.expr, ctx) for item in q.items]))
        else:
            sources = [(row, [evaluate(item.expr, Context(cols, row)) for item in q.items]) for row in rows]

    for name, descending in reversed(q.order_by):
        if name in names:
            idx = names.index(name)
            sources.sort(key=lambda s, idx=idx: _sort_key(s[1][idx]), reverse=descending)
        elif not aggregating and name in cols:
            idx = cols.index(name)
            sources.sort(key=lambda s, idx=idx: _sort_key(s[0][idx]), reverse=descending)
        else:
            raise QueryError(f"cannot order by {name!r}")

    out = [tuple(values) for _, values in sources]
    if q.limit is not None:
        out = out[: q.limit]
    return Result(names, out)
