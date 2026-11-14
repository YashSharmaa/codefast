import contextlib
import io
import os
import tempfile
import unittest

from csvq import QueryError, QuerySyntaxError, Table, query
from csvq.cli import main

from .support import CITIES_CSV, PEOPLE_CSV, join_tables


def rows(sql, t=None):
    return query(sql, t or join_tables()).rows


class JoinTests(unittest.TestCase):
    def test_inner_join_pairs_in_row_order(self):
        self.assertEqual(rows("SELECT name, country FROM people JOIN cities ON people.city = cities.city"),
                         [("Ada", "UK"), ("Bob", "France"), ("Cy", "UK"), ("Dee", "France")])
        r = query("SELECT p.name, q.name FROM people p JOIN people q ON p.city = q.city AND p.name < q.name",
                  join_tables())
        self.assertEqual(r.columns, ["name", "name"])
        self.assertEqual(r.rows, [("Ada", "Cy"), ("Bob", "Dee")])
        self.assertEqual(rows("SELECT p.name, q.name FROM people AS p JOIN people AS q ON p.age = q.age"),
                         [("Ada", "Ada"), ("Cy", "Cy"), ("Cy", "Eve"), ("Dee", "Dee"), ("Eve", "Cy"), ("Eve", "Eve")])

    def test_inner_keyword_aliases_and_where(self):
        self.assertEqual(rows("SELECT p.name, c.country FROM people AS p INNER JOIN cities c ON p.city = c.city "
                              "WHERE c.pop > 5"),
                         [("Ada", "UK"), ("Cy", "UK")])
        self.assertEqual(rows("select p.name from people p inner join cities c on p.city = c.city where pop < 3"),
                         [("Bob",), ("Dee",)])

    def test_left_join_keeps_unmatched_rows_in_place(self):
        r = query("SELECT name, country, pop FROM cities c LEFT JOIN people p ON c.city = p.city", join_tables())
        self.assertEqual(r.rows, [("Ada", "UK", 9), ("Cy", "UK", 9), ("Bob", "France", 2), ("Dee", "France", 2),
                                  (None, "Germany", 4)])
        self.assertEqual(rows("SELECT p.name, c.country FROM people p LEFT JOIN cities c ON p.city = c.city "
                              "WHERE c.country IS NULL"),
                         [("Eve", None)])
        r = query("SELECT * FROM one LEFT JOIN empty ON a = x", join_tables())
        self.assertEqual((r.columns, r.rows), (["x", "a", "b"], [(1, None, None)]))
        self.assertEqual(rows("SELECT * FROM one JOIN empty ON a = x"), [])

    def test_joins_apply_left_to_right(self):
        self.assertEqual(rows("SELECT p.name, k.continent FROM people p JOIN cities c ON p.city = c.city "
                              "JOIN countries k ON c.country = k.country WHERE p.age > 30"),
                         [("Ada", "Europe"), ("Dee", "Europe")])
        self.assertEqual(rows("SELECT c.city, k.continent, p.name FROM cities c LEFT JOIN countries k "
                              "ON k.country = c.country LEFT JOIN people p ON p.city = c.city AND p.age > 30"),
                         [("London", "Europe", "Ada"), ("Paris", "Europe", "Dee"), ("Berlin", None, None)])

    def test_star_and_qualified_star(self):
        r = query("SELECT * FROM people p JOIN cities c ON p.city = c.city LIMIT 1", join_tables())
        self.assertEqual(r.columns, ["name", "age", "city", "score", "city", "country", "pop"])
        self.assertEqual(r.rows, [("Ada", 36, "London", 9.5, "London", "UK", 9)])
        r = query("SELECT c.*, p.name, p.age + 1 FROM people p JOIN cities c ON p.city = c.city LIMIT 2",
                  join_tables())
        self.assertEqual(r.columns, ["city", "country", "pop", "name", "col5"])
        self.assertEqual(r.rows, [("London", "UK", 9, "Ada", 37), ("Paris", "France", 2, "Bob", None)])
        self.assertEqual(rows("SELECT people.* FROM people WHERE age = 41"), [("Dee", 41, "Paris", None)])

    def test_name_errors(self):
        bad = ["SELECT city FROM people JOIN cities ON people.city = cities.city",
               "SELECT people.name FROM people p",
               "SELECT p.nope FROM people p",
               "SELECT x.* FROM people",
               "SELECT name FROM people JOIN nowhere ON 1 = 1",
               "SELECT name FROM people JOIN people ON 1 = 1",
               "SELECT 1 FROM people p JOIN cities c ON k.country = c.country JOIN countries k ON 1 = 1",
               "SELECT 1 FROM people p JOIN cities c ON continent = 'Asia' JOIN countries k ON 1 = 1",
               "SELECT e.c FROM empty e JOIN one o ON e.a = o.x",
               "SELECT e.a FROM empty e JOIN one o ON e.a = o.y"]
        for sql in bad:
            with self.subTest(sql=sql):
                with self.assertRaises(QueryError):
                    query(sql, join_tables())

    def test_on_follows_where_rules(self):
        for sql in ["SELECT 1 FROM people p JOIN cities c ON p.age",
                    "SELECT 1 FROM people p JOIN cities c ON COUNT(*) > 1",
                    "SELECT 1 FROM people p LEFT JOIN cities c ON c.pop + 1"]:
            with self.subTest(sql=sql):
                with self.assertRaises(QueryError):
                    query(sql, join_tables())

    def test_group_by_over_a_join(self):
        self.assertEqual(rows("SELECT c.country, COUNT(*) AS n, SUM(p.age) FROM people p JOIN cities c "
                              "ON p.city = c.city GROUP BY country"),
                         [("UK", 2, 61), ("France", 2, 41)])
        self.assertEqual(rows("SELECT p.city, COUNT(*) FROM people p GROUP BY city"),
                         [("London", 2), ("Paris", 2), ("Rome", 1)])
        self.assertEqual(rows("SELECT city, MAX(p.age) FROM people p GROUP BY p.city"),
                         [("London", 36), ("Paris", 41), ("Rome", 25)])
        with self.assertRaises(QueryError):
            query("SELECT p.*, COUNT(*) FROM people p", join_tables())
        with self.assertRaises(QueryError):
            query("SELECT c.city, COUNT(*) FROM people p JOIN cities c ON p.city = c.city GROUP BY p.city",
                  join_tables())

    def test_order_by_with_qualifiers(self):
        self.assertEqual(rows("SELECT p.name FROM people p JOIN cities c ON p.city = c.city ORDER BY c.pop DESC, p.name"),
                         [("Ada",), ("Cy",), ("Bob",), ("Dee",)])
        self.assertEqual(rows("SELECT p.name, c.pop AS city FROM people p JOIN cities c ON p.city = c.city "
                              "ORDER BY city, name DESC"),
                         [("Dee", 2), ("Bob", 2), ("Cy", 9), ("Ada", 9)])
        for sql in ["SELECT city, COUNT(*) FROM people p GROUP BY city ORDER BY p.city",
                    "SELECT name FROM people JOIN cities ON people.city = cities.city ORDER BY city"]:
            with self.subTest(sql=sql):
                with self.assertRaises(QueryError):
                    query(sql, join_tables())

    def test_quoted_qualifiers_and_new_keywords(self):
        t = {"t": Table(["first name", "on"], [("Ann", 1)])}
        r = query('SELECT "my t"."first name", "my t"."on" FROM t AS "my t"', t)
        self.assertEqual((r.columns, r.rows), (["first name", "on"], [("Ann", 1)]))
        for sql in ["SELECT on FROM t", "SELECT 1 FROM t left", "SELECT 1 FROM t JOIN t u",
                    "SELECT 1 FROM t JOIN t u ON", "SELECT t. FROM t", "SELECT 1 FROM t LEFT t u ON 1 = 1"]:
            with self.subTest(sql=sql):
                with self.assertRaises(QuerySyntaxError):
                    query(sql, t)

    def test_left_join_comparison_on_missing_row(self):
        self.assertEqual(rows("SELECT p.name, c.country = 'UK' AS uk FROM people p LEFT JOIN cities c "
                              "ON p.city = c.city WHERE p.age < 30"),
                         [("Cy", True), ("Eve", False)])

    def test_cli_join(self):
        with tempfile.TemporaryDirectory() as d:
            people, cities = os.path.join(d, "people.csv"), os.path.join(d, "cities.csv")
            for path, text in ((people, PEOPLE_CSV), (cities, CITIES_CSV)):
                with open(path, "w", encoding="utf-8") as f:
                    f.write(text)
            out, err = io.StringIO(), io.StringIO()
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                code = main(["SELECT p.name, c.country FROM p LEFT JOIN c ON p.city = c.city WHERE p.score > 6.5",
                             "--table", f"p={people}", "--table", f"c={cities}"])
        self.assertEqual((code, out.getvalue(), err.getvalue()), (0, "name,country\nAda,UK\nBob,France\nCy,UK\n", ""))
