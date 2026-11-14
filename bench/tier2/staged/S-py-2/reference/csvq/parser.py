"""Recursive-descent parser for the query language."""

from .syntax import Aggregate, Binary, Call, Case, Column, IsNull, IsTruth, Join, Literal, Query, SelectItem, Source, Star, Unary
from .errors import ParseError
from .lexer import Token, tokenize

COMPARISONS = {"=", "!=", "<", "<=", ">", ">="}
AGGREGATES = {"COUNT", "SUM", "MIN", "MAX", "AVG"}


class _Parser:
    def __init__(self, text: str):
        self.text = text
        self.tokens = tokenize(text)
        self.i = 0

    @property
    def tok(self) -> Token:
        return self.tokens[self.i]

    def at(self, kind: str, value=None) -> bool:
        t = self.tok
        return t.kind == kind and (value is None or t.value == value)

    def peek_op(self, ahead: int, value: str) -> bool:
        """Is the token AHEAD places after the current one the operator VALUE?"""
        j = min(self.i + ahead, len(self.tokens) - 1)
        return self.tokens[j].kind == "OP" and self.tokens[j].value == value

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
            raise ParseError(f"expected {want}, found {got!r}", self.text, self.tok.pos)
        return t

    def query(self) -> Query:
        self.expect("KW", "SELECT")
        items = None
        if not self.accept("OP", "*"):
            items = [self.select_item()]
            while self.accept("OP", ","):
                items.append(self.select_item())
        self.expect("KW", "FROM")
        q = Query(items, self.source())
        while self.at("KW", "JOIN") or self.at("KW", "INNER") or self.at("KW", "LEFT"):
            q.joins.append(self.join())
        if self.accept("KW", "WHERE"):
            q.where = self.expr()
        if self.accept("KW", "GROUP"):
            self.expect("KW", "BY")
            q.group_by = [self.column_ref()]
            while self.accept("OP", ","):
                q.group_by.append(self.column_ref())
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
        if self.at("NAME") and self.peek_op(1, ".") and self.peek_op(2, "*"):
            t = self.expect("NAME")
            self.i += 2
            return SelectItem(Star(t.value, t.pos), None)
        expr = self.expr()
        alias = self.expect("NAME").value if self.accept("KW", "AS") else None
        return SelectItem(expr, alias)

    def source(self) -> Source:
        t = self.expect("NAME")
        alias = None
        if self.accept("KW", "AS") or self.at("NAME"):
            alias = self.expect("NAME").value
        return Source(t.value, alias, t.pos)

    def join(self) -> Join:
        left = self.accept("KW", "LEFT") is not None
        if not left:
            self.accept("KW", "INNER")
        self.expect("KW", "JOIN")
        source = self.source()
        self.expect("KW", "ON")
        return Join(left, source, self.expr())

    def column_ref(self) -> Column:
        """A column name, optionally qualified by a table name or alias."""
        first = self.expect("NAME")
        if self.accept("OP", "."):
            return Column(self.expect("NAME").value, first.value, first.pos)
        return Column(first.value, None, first.pos)

    def order_item(self) -> tuple[Column, bool]:
        name = self.column_ref()
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
            if self.accept("KW", "TRUE"):
                return IsTruth(node, True, negated)
            if self.accept("KW", "FALSE"):
                return IsTruth(node, False, negated)
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
        if self.accept("KW", "CASE"):
            return self.case()
        if self.accept("KW", "TRUE"):
            return Literal(True)
        if self.accept("KW", "FALSE"):
            return Literal(False)
        if t.kind == "NAME" and self.text[t.pos] != '"' and self.peek_op(1, "("):
            return self.call()
        if t.kind == "NAME":
            return self.column_ref()
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
        raise ParseError(f"unexpected {got!r}", self.text, t.pos)


    def case(self) -> Case:
        branches = []
        self.expect("KW", "WHEN")
        while True:
            condition = self.expr()
            self.expect("KW", "THEN")
            branches.append((condition, self.expr()))
            if not self.accept("KW", "WHEN"):
                break
        otherwise = self.expr() if self.accept("KW", "ELSE") else None
        self.expect("KW", "END")
        return Case(tuple(branches), otherwise)

    def call(self) -> Call:
        t = self.expect("NAME")
        self.expect("OP", "(")
        args = []
        if not self.accept("OP", ")"):
            args.append(self.expr())
            while self.accept("OP", ","):
                args.append(self.expr())
            self.expect("OP", ")")
        return Call(t.value.upper(), tuple(args), t.pos)


def parse(text: str) -> Query:
    return _Parser(text).query()
