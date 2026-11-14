import contextlib
import io
import os
import tempfile
import unittest

import csvq
import csvq.errors
from csvq import CsvqError, ParseError, QueryError, Table, query
from csvq.cli import main
from csvq.functions import call_function

from .support import PEOPLE_CSV, join_tables


class PositionTests(unittest.TestCase):
    def raised(self, sql, kind):
        with self.assertRaises(kind) as cm:
            query(sql, join_tables())
        return cm.exception

    def assertAt(self, sql, kind, position, line=None, column=None):
        e = self.raised(sql, kind)
        self.assertEqual(e.position, position, sql)
        self.assertEqual(e.sql, sql)
        if line is not None:
            self.assertEqual((e.line, e.column), (line, column), sql)
        self.assertEqual(str(e), f"{e.message} at line {e.line}, column {e.column}")

    def test_rename(self):
        self.assertFalse(hasattr(csvq, "QuerySyntaxError"))
        self.assertFalse(hasattr(csvq.errors, "QuerySyntaxError"))
        self.assertIs(csvq.ParseError, csvq.errors.ParseError)
        self.assertTrue(issubclass(ParseError, CsvqError))
        self.assertFalse(issubclass(ParseError, QueryError))
        self.assertFalse(issubclass(QueryError, ParseError))

    def test_parse_error_attributes(self):
        e = ParseError("bad thing", "SELECT\n  x\nFROM", 9)
        self.assertEqual((e.message, e.sql, e.position, e.line, e.column), ("bad thing", "SELECT\n  x\nFROM", 9, 2, 3))
        self.assertEqual(str(e), "bad thing at line 2, column 3")
        e = ParseError("m", "ab\n\ncd", 3)
        self.assertEqual((e.line, e.column), (2, 1))
        e = ParseError("m", "abc", 0)
        self.assertEqual((e.line, e.column, str(e)), (1, 1, "m at line 1, column 1"))
        e = ParseError("m", "ab\n", 3)
        self.assertEqual((e.line, e.column), (2, 1))

    def test_query_error_attributes(self):
        e = QueryError("oops")
        self.assertEqual((e.message, e.sql, e.position, e.line, e.column, str(e)), ("oops", None, None, None, None, "oops"))
        e = QueryError("unknown column 'nope'", "SELECT a,\n       nope", 17)
        self.assertEqual((e.line, e.column), (2, 8))
        self.assertEqual(str(e), "unknown column 'nope' at line 2, column 8")

    def test_characters_and_unterminated_text(self):
        self.assertAt("SELECT name FROM people;", ParseError, 23, 1, 24)
        self.assertAt("SELECT # FROM people", ParseError, 7)
        self.assertAt("SELECT 'abc FROM people", ParseError, 7)
        self.assertAt('SELECT name, "na me FROM people', ParseError, 13)
        sql = "SELECT name\nFROM people\nWHERE city = 'Rome"
        self.assertAt(sql, ParseError, sql.index("'"), 3, 14)

    def test_tokens_that_do_not_fit(self):
        self.assertAt("SELECT count FROM people", ParseError, 13)
        self.assertAt("SELECT name FROM people LIMIT -1", ParseError, 30)
        sql = "SELECT name FROM people WHERE age = 1 = 2"
        self.assertAt(sql, ParseError, sql.rindex("="))
        self.assertAt("FROM people SELECT name", ParseError, 0, 1, 1)
        self.assertAt("SELECT * , name FROM people", ParseError, 9)
        self.assertAt("SELECT name FROM people ORDER name", ParseError, 30)
        self.assertAt("SELECT (age FROM people", ParseError, 12)

    def test_query_ends_too_early(self):
        self.assertAt("SELECT name\nFROM", ParseError, 16, 2, 5)
        sql = "SELECT name FROM people WHERE"
        self.assertAt(sql, ParseError, len(sql))
        self.assertAt("SELECT", ParseError, 6)

    def test_new_syntax_positions(self):
        sql = "SELECT p. FROM people p"
        self.assertAt(sql, ParseError, sql.index("FROM"))
        sql = "SELECT name FROM people JOIN cities"
        self.assertAt(sql, ParseError, len(sql))
        sql = "SELECT name FROM people LEFT cities ON 1 = 1"
        self.assertAt(sql, ParseError, sql.index("cities"))
        sql = "SELECT CASE WHEN TRUE THEN 1\nFROM people"
        self.assertAt(sql, ParseError, sql.index("FROM"), 2, 1)
        sql = "SELECT UPPER(name FROM people"
        self.assertAt(sql, ParseError, sql.index("FROM"))

    def test_unknown_names(self):
        self.assertAt("SELECT name, nope FROM people", QueryError, 13, 1, 14)
        sql = "SELECT p.name\nFROM people AS p\nWHERE p.agee > 1"
        self.assertAt(sql, QueryError, sql.index("p.agee"), 3, 7)
        self.assertAt("SELECT 1 FROM nowhere", QueryError, 14)
        sql = "SELECT 1 FROM people p LEFT JOIN nowhere n ON p.age = n.age"
        self.assertAt(sql, QueryError, sql.index("nowhere"))
        self.assertAt("SELECT x.* FROM people", QueryError, 7)
        sql = "SELECT name FROM people WHERE q.age > 1"
        self.assertAt(sql, QueryError, sql.index("q.age"))
        sql = "SELECT a FROM empty ORDER BY \"no pe\""
        self.assertAt(sql, QueryError, sql.index('"no pe"'))

    def test_ambiguous_grouping_and_order_positions(self):
        sql = "SELECT p.name FROM people p JOIN cities c ON p.city = c.city WHERE city = 'x'"
        self.assertAt(sql, QueryError, sql.index("city = 'x'"))
        sql = "SELECT city,\n  MAX(age), age FROM people GROUP BY city"
        self.assertAt(sql, QueryError, sql.rindex("age FROM"), 2, 13)
        sql = "SELECT city, COUNT(*) FROM people GROUP BY city ORDER BY age"
        self.assertAt(sql, QueryError, sql.rindex("age"))
        sql = "SELECT city, COUNT(*) FROM people p GROUP BY city ORDER BY p.city"
        self.assertAt(sql, QueryError, sql.rindex("p.city"))

    def test_function_name_positions(self):
        sql = "SELECT 1,\n  nope(name) FROM people"
        self.assertAt(sql, QueryError, sql.index("nope"), 2, 3)
        self.assertAt("SELECT UPPER(name, 1) FROM people", QueryError, 7)
        sql = "SELECT a FROM empty WHERE coalesce() IS NULL"
        self.assertAt(sql, QueryError, sql.index("coalesce"))

    def test_other_errors_have_no_position(self):
        for sql in ["SELECT name + 1 FROM people", "SELECT name FROM people WHERE score",
                    "SELECT 1 FROM people JOIN people ON 1 = 1", "SELECT SUBSTR(name, 0) FROM people",
                    "SELECT name FROM people WHERE COUNT(*) > 1", "SELECT * FROM people GROUP BY city"]:
            with self.subTest(sql=sql):
                e = self.raised(sql, QueryError)
                self.assertEqual((e.position, e.line, e.column), (None, None, None))
                self.assertEqual(str(e), e.message)
        for make in [lambda: Table.from_csv("a,a\n1,2\n"), lambda: call_function("nope", [])]:
            with self.assertRaises(QueryError) as cm:
                make()
            self.assertIsNone(cm.exception.position)
            self.assertEqual(str(cm.exception), cm.exception.message)

    def test_cli_shows_the_place(self):
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "people.csv")
            with open(path, "w", encoding="utf-8") as f:
                f.write(PEOPLE_CSV)
            results = []
            for sql in ["SELECT nope FROM p", "SELECT name\nFROM p\nWHERE"]:
                out, err = io.StringIO(), io.StringIO()
                with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                    code = main([sql, "--table", f"p={path}"])
                results.append((code, out.getvalue(), err.getvalue()))
        for (code, out, err), end in zip(results, [" at line 1, column 8\n", " at line 3, column 6\n"]):
            self.assertEqual((code, out), (1, ""))
            self.assertTrue(err.startswith("error: "), err)
            self.assertTrue(err.endswith(end), err)
            self.assertEqual(len(err.splitlines()), 1)
