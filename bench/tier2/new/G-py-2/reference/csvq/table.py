"""In-memory tables and CSV loading."""

import csv
import io
import re

from .errors import QueryError

_INT_RE = re.compile(r"-?[0-9]+")
_FLOAT_RE = re.compile(r"-?[0-9]+\.[0-9]+")


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
        records = [r for r in csv.reader(io.StringIO(text)) if r]
        if not records:
            raise QueryError("CSV text has no header row")
        header = records[0]
        if len(set(header)) != len(header):
            raise QueryError("CSV header has duplicate column names")
        rows = []
        for number, record in enumerate(records[1:], start=2):
            if len(record) != len(header):
                raise QueryError(f"CSV row {number} has {len(record)} fields, expected {len(header)}")
            rows.append(tuple(convert_value(field) for field in record))
        return cls(header, rows)

    def __repr__(self):
        return f"Table(columns={self.columns!r}, rows={len(self.rows)})"
