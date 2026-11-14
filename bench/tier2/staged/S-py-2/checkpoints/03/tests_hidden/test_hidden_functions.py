import contextlib
import io
import os
import tempfile
import unittest

from csvq import QueryError, QuerySyntaxError, Table, query
from csvq.cli import main
from csvq.functions import call_function

from .support import PEOPLE_CSV, tables


def rows(sql, t=None):
    return query(sql, t or tables()).rows


def one(sql):
    (row,) = rows(sql + " FROM one")
    return row


class FunctionTests(unittest.TestCase):
    def test_upper_lower_length_trim(self):
        self.assertEqual(rows("SELECT UPPER(name), lower(city), Length(city), TRIM(' \tA b  ') FROM people LIMIT 2"),
                         [("ADA", "london", 6, "A b"), ("BOB", "paris", 5, "A b")])
        self.assertEqual(rows("SELECT name FROM people WHERE LENGTH(name) = 2"), [("Cy",)])

    def test_substr(self):
        self.assertEqual(one("SELECT SUBSTR('abcdef', 2), SUBSTR('abcdef', 2, 3), SUBSTR('abc', 5), "
                             "SUBSTR('abc', 1, 0), SUBSTR('abc', 3, 10)"),
                         ("bcdef", "bcd", "", "", "c"))
        for sql in ["SELECT SUBSTR('abc', 0) FROM one", "SELECT SUBSTR('abc', 1, -1) FROM one",
                    "SELECT SUBSTR('abc', 1.0) FROM one", "SELECT SUBSTR(5, 1) FROM one"]:
            with self.subTest(sql=sql):
                with self.assertRaises(QueryError):
                    query(sql, tables())

    def test_replace_and_instr(self):
        self.assertEqual(one("SELECT REPLACE('banana', 'an', 'AN'), REPLACE('abc', '', 'x'), REPLACE('aaa', 'a', ''), "
                             "INSTR('banana', 'na'), INSTR('abc', 'z'), INSTR('abc', '')"),
                         ("bANANa", "abc", "", 3, 0, 1))

    def test_concat(self):
        self.assertEqual(rows("SELECT CONCAT(name, '-', age, '/', score) FROM people WHERE city = 'Paris'"),
                         [("Bob-/7",), ("Dee-41/",)])
        self.assertEqual(rows("SELECT CONCAT(name, age > 30) FROM people LIMIT 2"), [("Adatrue",), ("Bobfalse",)])
        self.assertEqual(one("SELECT CONCAT(NULL, NULL), CONCAT(1.5), CONCAT(x)"), ("", "1.5", "1"))

    def test_abs_and_round(self):
        a, b, c, d, e, f, g, h = one("SELECT ABS(-3), ABS(-2.5), ROUND(2.5), ROUND(-2.5), ROUND(8.25, 1), "
                                     "ROUND(-0.125, 2), ROUND(7, 1), ROUND(9.4)")
        self.assertEqual((a, b, c, d, e, f, g, h), (3, 2.5, 3, -3, 8.3, -0.13, 7.0, 9))
        self.assertIs(type(a), int)
        self.assertIs(type(c), int)
        self.assertIs(type(g), float)
        self.assertIs(type(h), int)
        with self.assertRaises(QueryError):
            query("SELECT ROUND(1.5, -1) FROM one", tables())

    def test_coalesce_and_nullif(self):
        self.assertEqual(rows("SELECT COALESCE(age, score, -1), COALESCE(score, 'none'), NULLIF(city, 'Paris') "
                              "FROM people WHERE name > 'Ac' AND name < 'E'"),
                         [(36, 9.5, "London"), (7, 7, None), (25, 8.25, "London"), (41, "none", None)])
        self.assertEqual(one("SELECT NULLIF(1, 1.0), NULLIF(2, 1), COALESCE(NULL), NULLIF(NULL, 1)"),
                         (None, 2, None, None))
        with self.assertRaises(QueryError):
            query("SELECT NULLIF(1, 'a') FROM one", tables())

    def test_null_arguments(self):
        self.assertEqual(rows("SELECT UPPER(score), ABS(score), SUBSTR(name, score), ROUND(age, score), LENGTH(NULL), "
                              "REPLACE(name, NULL, 'x') FROM people WHERE name = 'Dee'"),
                         [(None, None, None, None, None, None)])

    def test_argument_type_errors(self):
        for expr in ["UPPER(age)", "LENGTH(1)", "ABS(name)", "ROUND('x')", "REPLACE(name, 1, 'a')",
                     "INSTR(1, 'a')", "TRIM(age > 1)", "ABS(age > 1)", "ROUND(score, 1.0)"]:
            with self.subTest(expr=expr):
                with self.assertRaises(QueryError):
                    query(f"SELECT {expr} FROM people WHERE name = 'Ada'", tables())

    def test_unknown_function_and_argument_count_without_rows(self):
        for sql in ["SELECT NOPE(a) FROM empty", "SELECT UPPER(a, b) FROM empty", "SELECT UPPER() FROM empty",
                    "SELECT ROUND(a, 1, 2) FROM empty", "SELECT COALESCE() FROM empty", "SELECT CONCAT() FROM empty",
                    "SELECT a FROM empty WHERE SUBSTR(a) = 'x'", "SELECT NULLIF(a) FROM empty"]:
            with self.subTest(sql=sql):
                with self.assertRaises(QueryError):
                    query(sql, tables())

    def test_function_names_are_not_keywords(self):
        t = {"t": Table(["upper", "x"], [("a", "b")])}
        r = query("SELECT upper, Upper(x), uPPer(upper) FROM t", t)
        self.assertEqual((r.columns, r.rows), (["upper", "col2", "col3"], [("a", "B", "A")]))
        with self.assertRaises(QuerySyntaxError):
            query('SELECT "upper"(x) FROM t', t)

    def test_functions_and_aggregates(self):
        r = query("SELECT city, UPPER(MIN(name)), SUM(ABS(age - 30)) AS gap, ROUND(AVG(score), 1) FROM people "
                  "GROUP BY city", tables())
        self.assertEqual(r.columns, ["city", "col2", "gap", "col4"])
        self.assertEqual(r.rows, [("London", "ADA", 11, 8.9), ("Paris", "BOB", 11, 7.0), ("Rome", "EVE", 5, 6.0)])
        self.assertEqual(rows("SELECT LOWER(city) AS c, COUNT(*) FROM people GROUP BY city ORDER BY c DESC LIMIT 1"),
                         [("rome", 1)])
        for sql in ["SELECT UPPER(city), COUNT(*) FROM people GROUP BY name", "SELECT COUNT(*) FROM people WHERE ABS(SUM(age)) > 1"]:
            with self.subTest(sql=sql):
                with self.assertRaises(QueryError):
                    query(sql, tables())

    def test_call_function(self):
        self.assertEqual(call_function("upper", ["ab"]), "AB")
        self.assertEqual(call_function("Round", [2.5]), 3)
        self.assertEqual(call_function("COALESCE", [None, None, 2]), 2)
        self.assertIsNone(call_function("length", [None]))
        for name, args in [("nope", []), ("UPPER", []), ("ABS", ["x"]), ("SUBSTR", ["abc", 0])]:
            with self.subTest(name=name, args=args):
                with self.assertRaises(QueryError):
                    call_function(name, args)

    def test_cli_output(self):
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "people.csv")
            with open(path, "w", encoding="utf-8") as f:
                f.write(PEOPLE_CSV)
            out, err = io.StringIO(), io.StringIO()
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                code = main(["SELECT CONCAT(age) AS a, UPPER(name) AS n FROM p WHERE LENGTH(name) = 3", "--table",
                             f"p={path}"])
        self.assertEqual((code, out.getvalue(), err.getvalue()), (0, "a,n\n36,ADA\n,BOB\n41,DEE\n25,EVE\n", ""))
