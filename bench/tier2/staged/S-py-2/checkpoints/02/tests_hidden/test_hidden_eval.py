import unittest

from csvq import QueryError, Table, query

from .support import tables


def rows(sql, t=None):
    return query(sql, t or tables()).rows


class EvaluationTests(unittest.TestCase):
    def test_select_star(self):
        r = query("SELECT * FROM people", tables())
        self.assertEqual(r.columns, ["name", "age", "city", "score"])
        self.assertEqual(r.rows[1], ("Bob", None, "Paris", 7))
        self.assertEqual(len(r.rows), 5)
        self.assertIsInstance(r.rows, list)
        self.assertIsInstance(r.rows[0], tuple)

    def test_null_comparisons_are_false_and_not_applies(self):
        self.assertEqual(rows("SELECT name FROM people WHERE age != 25"), [("Ada",), ("Dee",)])
        self.assertEqual(rows("SELECT name FROM people WHERE NOT age = 25"), [("Ada",), ("Bob",), ("Dee",)])
        self.assertEqual(rows("SELECT name FROM people WHERE age = NULL"), [])

    def test_is_null(self):
        self.assertEqual(rows("SELECT name FROM people WHERE score IS NULL OR age IS NULL"), [("Bob",), ("Dee",)])
        self.assertEqual(len(rows("SELECT name FROM people WHERE age IS NOT NULL")), 4)

    def test_precedence(self):
        r = rows("SELECT 2 + 3 * 4 AS a, (2 + 3) * 4 AS b, -2 * 3 AS c, 10 - 4 - 3 AS d, 7 / 2 AS e, 2 - -1 AS f FROM one")
        self.assertEqual(r, [(14, 20, -6, 3, 3.5, 3)])
        self.assertEqual(rows("SELECT name FROM people WHERE city = 'Rome' OR city = 'Paris' AND age > 30"),
                         [("Dee",), ("Eve",)])
        self.assertEqual(rows("SELECT name FROM people WHERE NOT city = 'London' AND age < 30"), [("Eve",)])

    def test_arithmetic_types(self):
        (a, b, c, d, e, f), = rows("SELECT x * 3, x + 0.5, 6 / 3, x / 0, x + NULL, x - 1.0 FROM one")
        self.assertEqual((a, b, c, d, e, f), (3, 1.5, 2.0, None, None, 0.0))
        self.assertIs(type(a), int)
        self.assertIs(type(c), float)
        self.assertIs(type(f), float)

    def test_string_comparison_by_code_point(self):
        self.assertEqual(rows("SELECT name FROM people WHERE name < 'Cz' AND name >= 'B'"), [("Bob",), ("Cy",)])
        t = {"t": Table(["s"], [("a",), ("B",), ("b",)])}
        self.assertEqual(rows("SELECT s FROM t WHERE s < 'a'", t), [("B",)])

    def test_type_errors(self):
        for sql in ["SELECT name FROM people WHERE name > 3", "SELECT name + 1 FROM people",
                    "SELECT name FROM people WHERE age", "SELECT name FROM people WHERE age > 1 AND name",
                    "SELECT -city FROM people", "SELECT name FROM people WHERE NOT score"]:
            with self.subTest(sql=sql):
                with self.assertRaises(QueryError):
                    query(sql, tables())

    def test_unknown_names_even_without_rows(self):
        for sql in ["SELECT a FROM nowhere", "SELECT c FROM empty", "SELECT a FROM empty WHERE c = 1",
                    "SELECT a FROM empty GROUP BY c", "SELECT a FROM empty ORDER BY c",
                    "SELECT COUNT(c) FROM empty"]:
            with self.subTest(sql=sql):
                with self.assertRaises(QueryError):
                    query(sql, tables())

    def test_output_column_names(self):
        r = query("SELECT name AS n, age, age * 2, 'x', COUNT(*) AS c FROM people GROUP BY name, age", tables())
        self.assertEqual(r.columns, ["n", "age", "col3", "col4", "c"])
        self.assertEqual(r.rows[0], ("Ada", 36, 72, "x", 1))
        r = query("SELECT x, x FROM one", tables())
        self.assertEqual(r.columns, ["x", "x"])
