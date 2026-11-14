"""Error types for csvq."""


class CsvqError(Exception):
    """Base class of every error csvq raises."""


class QuerySyntaxError(CsvqError):
    """The query text does not fit the grammar."""


class QueryError(CsvqError):
    """The query is well formed but cannot be run, or the data is bad."""
