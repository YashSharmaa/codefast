"""Error types for csvq."""


def place(sql: str, position: int) -> tuple[int, int]:
    """The 1-based line and column of a 0-based position in sql."""
    before = sql[:position]
    line_start = before.rfind("\n") + 1
    return before.count("\n") + 1, position - line_start + 1


class CsvqError(Exception):
    """Base class of every error csvq raises."""


class _Located(CsvqError):
    """An error that may name a place in the query text."""

    def __init__(self, message: str, sql: str | None, position: int | None):
        self.message = message
        self.sql = sql
        self.position = position
        if position is None:
            self.line = self.column = None
            super().__init__(message)
        else:
            self.line, self.column = place(sql, position)
            super().__init__(f"{message} at line {self.line}, column {self.column}")


class ParseError(_Located):
    """The query text does not fit the grammar."""

    def __init__(self, message: str, sql: str, position: int):
        super().__init__(message, sql, position)


class QueryError(_Located):
    """The query is well formed but cannot be run, or the data is bad."""

    def __init__(self, message: str, sql: str | None = None, position: int | None = None):
        super().__init__(message, sql, position)
