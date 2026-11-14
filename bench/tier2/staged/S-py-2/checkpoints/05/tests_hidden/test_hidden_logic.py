import unittest

from csvq import ParseError, QueryError, Table, query

from .support import join_tables, tables


def rows(sql, t=None):
    return query(sql, t or tables()).rows


def truth_tables():
    # v = 1 is TRUE, FALSE and NULL on the three rows
    return {"t": Table(["v"], [(1,), (0,), (None,)])}


class LogicTests(unittest.TestCase):
    def test_comparisons_with_null_give_null(self):
        self.assertEqual(rows("SELECT age = 25, age != 25, age <> 25, age < 1, age >= 1 FROM people WHERE name = 'Bob'"),
                         [(None, None, None, None, None)])
        self.assertEqual(rows("SELECT NULL = NULL, NULL != NULL, NULL = 'x', 1 < NULL FROM one"),
                         [(None, None, None, None)])

    def test_where_keeps_only_true(self):
        self.assertEqual(rows("SELECT name FROM people WHERE NOT age = 25"), [("Ada",), ("Dee",)])
        self.assertEqual(rows("SELECT name FROM people WHERE NOT score > 7"), [("Bob",), ("Eve",)])
        self.assertEqual(rows("SELECT name FROM people WHERE NOT (age > 30 AND score > 9)"),
                         [("Bob",), ("Cy",), ("Eve",)])
        self.assertEqual(rows("SELECT name FROM people WHERE NOT (age > 30 OR score > 9)"), [("Cy",), ("Eve",)])

    def test_and_or_truth_tables(self):
        r = query("SELECT x.v, y.v, x.v = 1 AND y.v = 1, x.v = 1 OR y.v = 1 FROM t x JOIN t y ON 1 = 1", truth_tables())
        T, F, N = True, False, None
        self.assertEqual(r.rows, [(1, 1, T, T), (1, 0, F, T), (1, N, N, T),
                                  (0, 1, F, T), (0, 0, F, F), (0, N, F, N),
                                  (N, 1, N, T), (N, 0, F, N), (N, N, N, N)])

    def test_literals_and_not(self):
        r = query("SELECT NOT NULL, TRUE, false, NOT FALSE, NOT v = 1 FROM t", truth_tables())
        self.assertEqual(r.rows, [(None, True, False, True, False), (None, True, False, True, True),
                                  (None, True, False, True, None)])
        self.assertIs(r.rows[0][1], True)
        self.assertEqual(r.columns, ["col1", "col2", "col3", "col4", "col5"])

    def test_logic_type_errors(self):
        for sql in ["SELECT NULL AND 1 FROM one", "SELECT 'a' OR TRUE FROM one", "SELECT NOT 5 FROM one",
                    "SELECT TRUE OR name FROM people", "SELECT FALSE AND age FROM people",
                    "SELECT name FROM people WHERE score", "SELECT 1 FROM one o JOIN one p ON o.x"]:
            with self.subTest(sql=sql):
                with self.assertRaises(QueryError):
                    query(sql, tables())

    def test_is_true_and_is_false(self):
        r = query("SELECT (v = 1) IS TRUE, (v = 1) IS NOT TRUE, (v = 1) IS FALSE, (v = 1) IS NOT FALSE FROM t",
                  truth_tables())
        self.assertEqual(r.rows, [(True, False, False, True), (False, True, True, False), (False, True, False, True)])
        self.assertEqual(rows("SELECT name FROM people WHERE (age > 30) IS NOT TRUE"), [("Bob",), ("Cy",), ("Eve",)])
        with self.assertRaises(QueryError):
            query("SELECT name FROM people WHERE age IS TRUE", tables())
        for sql in ["SELECT name FROM people WHERE age > 30 IS TRUE", "SELECT name FROM people WHERE age IS MAYBE",
                    "SELECT name FROM people WHERE age IS NOT"]:
            with self.subTest(sql=sql):
                with self.assertRaises(ParseError):
                    query(sql, tables())

    def test_case(self):
        r = query("SELECT name, CASE WHEN age > 30 THEN 'old' WHEN age IS NULL THEN '?' ELSE 'young' END, "
                  "CASE WHEN score > 8 THEN 1 WHEN score > 6 THEN 2 END AS band FROM people", tables())
        self.assertEqual(r.columns, ["name", "col2", "band"])
        self.assertEqual(r.rows, [("Ada", "old", 1), ("Bob", "?", 2), ("Cy", "young", 1), ("Dee", "old", None),
                                  ("Eve", "young", None)])
        self.assertEqual(rows("SELECT CASE WHEN NULL THEN 1 WHEN FALSE THEN 2 ELSE 3 END, CASE WHEN NULL THEN 1 END FROM one"),
                         [(3, None)])

    def test_case_evaluates_only_what_it_needs(self):
        self.assertEqual(rows("SELECT CASE WHEN TRUE THEN 1 ELSE -name END FROM people LIMIT 2"), [(1,), (1,)])
        self.assertEqual(rows("SELECT CASE WHEN name = 'Ada' THEN 0 WHEN name + 1 > 0 THEN 1 END FROM people "
                              "WHERE name = 'Ada'"),
                         [(0,)])
        with self.assertRaises(QueryError):
            query("SELECT CASE WHEN name THEN 1 END FROM people", tables())
        for sql in ["SELECT CASE END FROM one", "SELECT CASE WHEN TRUE THEN 1 FROM one", "SELECT CASE WHEN TRUE 1 END FROM one",
                    "SELECT CASE ELSE 1 END FROM one"]:
            with self.subTest(sql=sql):
                with self.assertRaises(ParseError):
                    query(sql, tables())

    def test_case_with_aggregates(self):
        self.assertEqual(rows("SELECT city, CASE WHEN COUNT(*) > 1 THEN 'many' ELSE 'one' END AS k, "
                              "SUM(CASE WHEN age > 30 THEN 1 ELSE 0 END) FROM people GROUP BY city"),
                         [("London", "many", 1), ("Paris", "many", 1), ("Rome", "one", 0)])
        with self.assertRaises(QueryError):
            query("SELECT CASE WHEN age > 1 THEN 1 END, COUNT(*) FROM people", tables())

    def test_boolean_comparisons(self):
        self.assertEqual(rows("SELECT TRUE = TRUE, TRUE <> FALSE, FALSE != FALSE FROM one"), [(True, True, False)])
        self.assertEqual(rows("SELECT name, (age > 30) = TRUE FROM people LIMIT 2"), [("Ada", True), ("Bob", None)])
        for sql in ["SELECT TRUE < FALSE FROM one", "SELECT TRUE = 1 FROM one", "SELECT FALSE = 'x' FROM one",
                    "SELECT (x > 0) >= TRUE FROM one"]:
            with self.subTest(sql=sql):
                with self.assertRaises(QueryError):
                    query(sql, tables())

    def test_on_with_null(self):
        self.assertEqual(rows("SELECT p.name, q.name FROM people p LEFT JOIN people q "
                              "ON NOT p.age != q.age AND p.name < q.name", join_tables()),
                         [("Ada", None), ("Bob", None), ("Cy", "Eve"), ("Dee", None), ("Eve", None)])

    def test_null_conditions_in_output_and_order(self):
        out = query("SELECT name, age > 30 AS old FROM people ORDER BY old, name", tables()).to_csv()
        self.assertEqual(out, "name,old\nBob,\nCy,false\nEve,false\nAda,true\nDee,true\n")

    def test_new_keywords(self):
        t = {"t": Table(["end", "true"], [(1, 2)])}
        self.assertEqual(query('SELECT "end", "true", true FROM t', t).rows, [(1, 2, True)])
        for sql in ["SELECT end FROM t", "SELECT case FROM t", "SELECT 1 AS then FROM t", "SELECT x FROM t AS when"]:
            with self.subTest(sql=sql):
                with self.assertRaises(ParseError):
                    query(sql, t)
