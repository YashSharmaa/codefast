"""Recursive-descent parser for the query language."""

from .syntax import Aggregate, Binary, Column, IsNull, Literal, Query, SelectItem, Unary
from .errors import QuerySyntaxError
from .lexer import Token, tokenize

COMPARISONS = {"=", "!=", "<", "<=", ">", ">="}
AGGREGATES = {"COUNT", "SUM", "MIN", "MAX", "AVG"}


class _Parser:
    def __init__(self, text: str):
        self.tokens = tokenize(text)
        self.i = 0

    @property
    def tok(self) -> Token:
        return self.tokens[self.i]

    def at(self, kind: str, value=None) -> bool:
        t = self.tok
        return t.kind == kind and (value is None or t.value == value)

    def accept(self, kind: str, value=None):
        if self.at(kind, value):
            t = self.tok
            self.i += 1
            return t
        return None

    def expect(self, kind: str, value=None) -> Token:
        t = self.accept(kind, value)
        if t is None:
            want = value if value is not None else kind
            got = self.tok.value if self.tok.kind != "EOF" else "end of query"
            raise QuerySyntaxError(f"expected {want} at {self.tok.pos}, found {got!r}")
        return t

    def query(self) -> Query:
        self.expect("KW", "SELECT")
        items = None
        if not self.accept("OP", "*"):
            items = [self.select_item()]
            while self.accept("OP", ","):
                items.append(self.select_item())
        self.expect("KW", "FROM")
        q = Query(items, self.expect("NAME").value)
        if self.accept("KW", "WHERE"):
            q.where = self.expr()
        if self.accept("KW", "GROUP"):
            self.expect("KW", "BY")
            q.group_by = [self.expect("NAME").value]
            while self.accept("OP", ","):
                q.group_by.append(self.expect("NAME").value)
        if self.accept("KW", "ORDER"):
            self.expect("KW", "BY")
            q.order_by = [self.order_item()]
            while self.accept("OP", ","):
                q.order_by.append(self.order_item())
        if self.accept("KW", "LIMIT"):
            q.limit = self.expect("INT").value
        self.expect("EOF")
        return q

    def select_item(self) -> SelectItem:
        expr = self.expr()
        alias = self.expect("NAME").value if self.accept("KW", "AS") else None
        return SelectItem(expr, alias)

    def order_item(self) -> tuple[str, bool]:
        name = self.expect("NAME").value
        if self.accept("KW", "DESC"):
            return name, True
        self.accept("KW", "ASC")
        return name, False

    def expr(self):
        node = self.and_expr()
        while self.accept("KW", "OR"):
            node = Binary("OR", node, self.and_expr())
        return node

    def and_expr(self):
        node = self.not_expr()
        while self.accept("KW", "AND"):
            node = Binary("AND", node, self.not_expr())
        return node

    def not_expr(self):
        if self.accept("KW", "NOT"):
            return Unary("NOT", self.not_expr())
        return self.comparison()

    def comparison(self):
        node = self.additive()
        if self.tok.kind == "OP" and self.tok.value in COMPARISONS:
            op = self.expect("OP").value
            return Binary(op, node, self.additive())
        if self.accept("KW", "IS"):
            negated = self.accept("KW", "NOT") is not None
            self.expect("KW", "NULL")
            return IsNull(node, negated)
        return node

    def additive(self):
        node = self.term()
        while self.tok.kind == "OP" and self.tok.value in ("+", "-"):
            op = self.expect("OP").value
            node = Binary(op, node, self.term())
        return node

    def term(self):
        node = self.unary()
        while self.tok.kind == "OP" and self.tok.value in ("*", "/"):
            op = self.expect("OP").value
            node = Binary(op, node, self.unary())
        return node

    def unary(self):
        if self.accept("OP", "-"):
            return Unary("-", self.unary())
        return self.primary()

    def primary(self):
        t = self.tok
        if t.kind in ("INT", "FLOAT", "STR"):
            self.i += 1
            return Literal(t.value)
        if self.accept("KW", "NULL"):
            return Literal(None)
        if t.kind == "NAME":
            self.i += 1
            return Column(t.value)
        if t.kind == "KW" and t.value in AGGREGATES:
            self.i += 1
            self.expect("OP", "(")
            if t.value == "COUNT" and self.accept("OP", "*"):
                arg = None
            else:
                arg = self.expr()
            self.expect("OP", ")")
            return Aggregate(t.value, arg)
        if self.accept("OP", "("):
            node = self.expr()
            self.expect("OP", ")")
            return node
        got = t.value if t.kind != "EOF" else "end of query"
        raise QuerySyntaxError(f"unexpected {got!r} at {t.pos}")


def parse(text: str) -> Query:
    return _Parser(text).query()
