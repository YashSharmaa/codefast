"""In-memory tables and CSV loading."""

import re

from .errors import QueryError

_INT_RE = re.compile(r"-?[0-9]+")
_FLOAT_RE = re.compile(r"-?[0-9]+\.[0-9]+")


def looks_converted(text: str) -> bool:
    """Would this text, read as an unquoted field, become something other than itself?"""
    return text == "" or bool(_INT_RE.fullmatch(text) or _FLOAT_RE.fullmatch(text))


def read_records(text: str) -> list[list[tuple[str, bool]]]:
    """Split CSV text into records of (field text, was quoted) pairs, RFC 4180 style."""
    records, record, field = [], [], []
    quoted = in_quotes = False
    i = 0
    while i < len(text):
        ch = text[i]
        if in_quotes:
            if ch == '"' and text.startswith('""', i):
                field.append('"')
                i += 1
            elif ch == '"':
                in_quotes = False
            else:
                field.append(ch)
        elif ch == '"' and not field and not quoted:
            in_quotes = quoted = True
        elif ch == ",":
            record.append(("".join(field), quoted))
            field, quoted = [], False
        elif ch in "\r\n":
            if text.startswith("\r\n", i):
                i += 1
            record.append(("".join(field), quoted))
            records.append(record)
            record, field, quoted = [], [], False
        else:
            field.append(ch)
        i += 1
    if in_quotes:
        raise QueryError("CSV text ends inside a quoted field")
    if record or field or quoted:
        record.append(("".join(field), quoted))
        records.append(record)
    return records


def convert_value(text: str):
    if text == "":
        return None
    if _INT_RE.fullmatch(text):
        return int(text)
    if _FLOAT_RE.fullmatch(text):
        return float(text)
    return text


class Table:
    def __init__(self, columns: list[str], rows: list[tuple]):
        self.columns = list(columns)
        self.rows = [tuple(row) for row in rows]

    def index_of(self, name: str) -> int:
        try:
            return self.columns.index(name)
        except ValueError:
            raise QueryError(f"unknown column {name!r}") from None

    @classmethod
    def from_csv(cls, text: str) -> "Table":
        records = [r for r in read_records(text) if r != [("", False)]]
        if not records:
            raise QueryError("CSV text has no header row")
        header = [field for field, _ in records[0]]
        if len(set(header)) != len(header):
            raise QueryError("CSV header has duplicate column names")
        rows = []
        for number, record in enumerate(records[1:], start=2):
            if len(record) != len(header):
                raise QueryError(f"CSV row {number} has {len(record)} fields, expected {len(header)}")
            rows.append(tuple(field if quoted else convert_value(field) for field, quoted in record))
        return cls(header, rows)

    def __repr__(self):
        return f"Table(columns={self.columns!r}, rows={len(self.rows)})"
