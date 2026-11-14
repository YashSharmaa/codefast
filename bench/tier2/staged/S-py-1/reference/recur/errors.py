"""Error types for recur."""


class RecurError(ValueError):
    """Raised for any malformed rule, date, schedule file or request.

    key is the rule key the problem concerns (such as "COUNT"), line the 1-based line of a
    schedule file. Each is None when it does not apply.
    """

    def __init__(self, message: str, key: str | None = None, line: int | None = None):
        super().__init__(message)
        self.key = key
        self.line = line
