# csvq: a small SQL query engine for CSV data

`csvq` runs read-only SQL `SELECT` queries over CSV tables held in memory. It is a Python
package with a command line. The project will be checked by tests written against the
interface below, so names, signatures, error types and output formats must match exactly.

## Package layout

| Module | Public names |
|---|---|
| `csvq/errors.py` | `CsvqError(Exception)`, `QuerySyntaxError(CsvqError)`, `QueryError(CsvqError)` |
| `csvq/table.py` | `Table` |
| `csvq/engine.py` | `Result`, `query(sql: str, tables: dict[str, Table]) -> Result` |
| `csvq/cli.py` | `main(argv: list[str]) -> int` |
| `csvq/__main__.py` | `python -m csvq ...` runs `main(sys.argv[1:])` and exits with its result |

`csvq/__init__.py` re-exports `CsvqError`, `QuerySyntaxError`, `QueryError`, `Table`, `Result`
and `query`. Split the lexer, parser and evaluator into further modules as you see fit.

`QuerySyntaxError` is raised for text that does not fit the grammar (including an unterminated
string or quoted name, an unexpected character, or text left over after the query). `QueryError` is raised for everything else that is wrong:
unknown tables or columns, type errors, misuse of aggregates, bad CSV.

## Tables

`Table(columns: list[str], rows: list[tuple])` holds column names and rows of values. Values
are `int`, `float`, `str` or `None`. `Table.from_csv(text: str) -> Table` reads CSV text
(comma separated, double-quote quoting as in RFC 4180, blank lines skipped). The first row is
the header. Every other field is converted: `-?[0-9]+` becomes an `int`, `-?[0-9]+\.[0-9]+`
becomes a `float`, the empty string becomes `None`, anything else stays a `str` unchanged
(no trimming). `QueryError` for text with no header, duplicate column names, or a row whose
field count differs from the header.

## Query language

```
SELECT items FROM table [WHERE expr] [GROUP BY name, ...] [ORDER BY name [ASC|DESC], ...] [LIMIT n]
items  := * | expr [AS name], ...
expr   := literals, column names, ( expr ), unary -, * /, + -, comparisons, IS [NOT] NULL, NOT, AND, OR
```

Precedence from tightest: unary `-`, then `*` `/`, then `+` `-`, then comparisons (`=`, `!=`,
`<>`, `<`, `<=`, `>`, `>=`) and `IS NULL` / `IS NOT NULL`, then `NOT`, then `AND`, then `OR`.
Binary operators of the same level group left to right. Comparisons do not chain, so
`a = b = c` is a `QuerySyntaxError`.

- Keywords are case-insensitive: `SELECT FROM WHERE GROUP BY ORDER ASC DESC LIMIT AS AND OR
  NOT IS NULL COUNT SUM MIN MAX AVG`. Using one as a bare name is a `QuerySyntaxError`.
- Names are `[A-Za-z_][A-Za-z0-9_]*` and case-sensitive, or any text in double quotes (`""`
  inside means one quote), so `"first name"` names a column with a space.
- Literals: integers `123`, decimals `1.5`, strings `'it''s'` (`''` is one quote), `NULL`.
- Aggregates: `COUNT(*)`, `COUNT(expr)`, `SUM(expr)`, `MIN(expr)`, `MAX(expr)`, `AVG(expr)`.
- `LIMIT` takes an integer literal such as `0` or `10`. Anything else there (`-1`, `1.5`) is a
  `QuerySyntaxError`.

## Evaluation

- Arithmetic (`+ - * /` and unary `-`) works on numbers only. `int` with `int` gives `int` for `+ - *`, any `float`
  operand gives `float`, `/` always gives `float`. Division by zero gives `NULL`. Any `NULL`
  operand gives `NULL`. A string operand is a `QueryError`.
- Comparisons give `True` or `False`. Numbers compare numerically, strings by code point.
  Any comparison with a `NULL` operand is `False` (also for `!=`). Comparing a number with a
  string is a `QueryError`. `AND`, `OR` and `NOT` need boolean operands, else `QueryError`.
- `WHERE` keeps a row when its expression is `True`. A non-boolean result is a `QueryError`.
- Unknown tables and unknown columns are a `QueryError` even when the table has no rows.
- `SELECT *` lists all columns in table order. The grammar allows `*` only as the whole
  select list, and `SELECT *` with `GROUP BY` is a `QueryError`.

Aggregation happens when the select list contains an aggregate or the query has `GROUP BY`.
Rows are grouped by the `GROUP BY` columns (`NULL` equals `NULL` here), groups in order of
first appearance. Without `GROUP BY` all rows form one group, so a table with no rows still
gives one output row (`COUNT` 0, the other aggregates `NULL`). Outside aggregates a select item
may only use `GROUP BY` columns, else `QueryError`. Aggregates may sit inside expressions
(`MAX(a) - MIN(a)`) but not inside other aggregates or in `WHERE` (`QueryError`).

`COUNT(*)` counts rows, `COUNT(expr)` counts non-`NULL` values. `SUM` adds numbers (`int` if all
are `int`), `AVG` gives a `float`, `MIN` and `MAX` work on numbers or on strings. All four skip
`NULL`s, give `NULL` when nothing is left, and raise `QueryError` on values they cannot handle
(strings for `SUM` and `AVG`, a mix of numbers and strings for `MIN` and `MAX`).

Output column names: the alias when there is one, else the column name for a bare column,
else `colN` where N is the item's 1-based position in the select list.

`ORDER BY` names an output column (the first one with that name), or failing that, in a query
without aggregation, a column of the table. Anything else is a `QueryError`. Sorting is stable
and uses the keys in order. Within a key `NULL` comes first, then `False`, `True`, numbers and
strings. `DESC` reverses that order for its key but rows with equal keys keep their input
order. `LIMIT` is applied last.

## Results

`Result` has `columns: list[str]` and `rows: list[tuple]`. `Result.to_csv() -> str` writes
the header line and one line per row, each line ending in `\n`. Fields: `None` and the empty
string are empty, `True` and `False` are `true` and `false`, numbers use Python `str()`. A
field containing a comma, a double quote, `\r` or `\n` is wrapped in double quotes with inner
quotes doubled. No other field is quoted.

## Command line

`main(argv)` writes to `sys.stdout` and `sys.stderr` as they are at call time and returns the
exit status without calling `sys.exit`.

```
python -m csvq SQL --table NAME=PATH [--table NAME=PATH ...]
```

Each `--table` loads the CSV file at PATH (UTF-8) under NAME. On success it prints
`Result.to_csv()` exactly and returns 0. A `CsvqError` or an unreadable file prints
`error: <message>` as one line to stderr and returns 1. A usage error (no SQL, no `--table`, a
`--table` value without `=`) prints a message to stderr and returns 2.

## Constraints

Python 3.15, standard library only. Put your own tests anywhere except `tests_hidden/`, which
is reserved for the checking tests and will be replaced.
