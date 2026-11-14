import unittest

from csvq import QueryError, Table, query

from .support import tables


def rows(sql, t=None):
    return query(sql, t or tables()).rows


class AggregateTests(unittest.TestCase):
    def test_group_by_first_appearance(self):
        r = query("SELECT city, COUNT(*) AS n, COUNT(age) AS aged, SUM(age), AVG(age), MIN(name), MAX(score) "
                  "FROM people GROUP BY city", tables())
        self.assertEqual(r.rows, [("London", 2, 2, 61, 30.5, "Ada", 9.5),
                                  ("Paris", 2, 1, 41, 41.0, "Bob", 7),
                                  ("Rome", 1, 1, 25, 25.0, "Eve", 6)])

    def test_sum_and_avg_types(self):
        (s_age, s_score, avg), = rows("SELECT SUM(age), SUM(score), AVG(age) FROM people")
        self.assertEqual((s_age, s_score, avg), (127, 30.75, 31.75))
        self.assertIs(type(s_age), int)
        self.assertIs(type(s_score), float)
        self.assertIs(type(rows("SELECT AVG(x) FROM one")[0][0]), float)

    def test_no_rows_without_group_by_gives_one_row(self):
        self.assertEqual(rows("SELECT COUNT(*), COUNT(age), SUM(age), MAX(name), AVG(age) FROM people WHERE age > 100"),
                         [(0, 0, None, None, None)])
        self.assertEqual(rows("SELECT COUNT(*) FROM empty"), [(0,)])
        self.assertEqual(rows("SELECT a, COUNT(*) FROM empty GROUP BY a"), [])

    def test_min_max_strings_and_mixed(self):
        t = {"t": Table(["v", "w"], [("pear", 1), ("apple", "x"), (None, 3)])}
        self.assertEqual(rows("SELECT MIN(v), MAX(v), COUNT(v) FROM t", t), [("apple", "pear", 2)])
        for sql in ["SELECT MIN(w) FROM t", "SELECT SUM(v) FROM t", "SELECT AVG(v) FROM t"]:
            with self.subTest(sql=sql):
                with self.assertRaises(QueryError):
                    query(sql, t)

    def test_aggregate_misuse(self):
        for sql in ["SELECT name, COUNT(*) FROM people", "SELECT city, age FROM people GROUP BY city",
                    "SELECT name FROM people WHERE COUNT(*) > 1", "SELECT SUM(MAX(age)) FROM people",
                    "SELECT * FROM people GROUP BY city", "SELECT city, COUNT(*) FROM people GROUP BY city ORDER BY age"]:
            with self.subTest(sql=sql):
                with self.assertRaises(QueryError):
                    query(sql, tables())

    def test_aggregates_inside_expressions(self):
        self.assertEqual(rows("SELECT city, MAX(age) - MIN(age) AS spread, COUNT(*) * 10 FROM people GROUP BY city"),
                         [("London", 11, 20), ("Paris", 0, 20), ("Rome", 0, 10)])

    def test_null_group_key(self):
        self.assertEqual(rows("SELECT age, COUNT(*) FROM people GROUP BY age"),
                         [(36, 1), (None, 1), (25, 2), (41, 1)])

    def test_group_by_without_aggregates_and_where(self):
        self.assertEqual(rows("SELECT city FROM people WHERE age < 40 GROUP BY city"), [("London",), ("Rome",)])
