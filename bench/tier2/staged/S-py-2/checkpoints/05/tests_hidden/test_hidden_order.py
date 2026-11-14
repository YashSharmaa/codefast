import unittest

from csvq import QueryError, Result, Table, query

from .support import tables


def rows(sql, t=None):
    return query(sql, t or tables()).rows


class OrderTests(unittest.TestCase):
    def test_multiple_keys(self):
        self.assertEqual(rows("SELECT name FROM people ORDER BY city DESC, age"),
                         [("Eve",), ("Bob",), ("Dee",), ("Cy",), ("Ada",)])

    def test_desc_keeps_input_order_for_ties_and_puts_null_last(self):
        self.assertEqual(rows("SELECT name, age FROM people ORDER BY age DESC"),
                         [("Dee", 41), ("Ada", 36), ("Cy", 25), ("Eve", 25), ("Bob", None)])
        self.assertEqual(rows("SELECT name FROM people ORDER BY age ASC"),
                         [("Bob",), ("Cy",), ("Eve",), ("Ada",), ("Dee",)])

    def test_alias_and_unselected_column(self):
        self.assertEqual(rows("SELECT name AS who FROM people ORDER BY score DESC LIMIT 2"), [("Ada",), ("Cy",)])
        self.assertEqual(rows("SELECT age * -1 AS neg, name FROM people WHERE age IS NOT NULL ORDER BY neg, name"),
                         [(-41, "Dee"), (-36, "Ada"), (-25, "Cy"), (-25, "Eve")])
        with self.assertRaises(QueryError):
            query("SELECT name FROM people ORDER BY nothing", tables())

    def test_order_by_output_alias_shadows_table_column(self):
        self.assertEqual(rows("SELECT name, score AS age FROM people WHERE score IS NOT NULL ORDER BY age LIMIT 2"),
                         [("Eve", 6), ("Bob", 7)])

    def test_mixed_type_ranking(self):
        t = {"t": Table(["v"], [(None,), ("b",), (3,), ("a",), (1.5,)])}
        self.assertEqual(rows("SELECT v FROM t ORDER BY v", t), [(None,), (1.5,), (3,), ("a",), ("b",)])
        self.assertEqual(rows("SELECT name, age > 30 AS old FROM people ORDER BY old DESC, name LIMIT 3"),
                         [("Ada", True), ("Dee", True), ("Cy", False)])

    def test_limit(self):
        self.assertEqual(rows("SELECT name FROM people LIMIT 0"), [])
        self.assertEqual(query("SELECT name FROM people LIMIT 0", tables()).to_csv(), "name\n")
        self.assertEqual(rows("SELECT name FROM people ORDER BY name DESC LIMIT 2"), [("Eve",), ("Dee",)])
        self.assertEqual(len(rows("SELECT name FROM people LIMIT 50")), 5)


class ResultTests(unittest.TestCase):
    def test_to_csv_formatting(self):
        r = Result(["a", "b,c"], [(None, True), (2.5, False), ('say "hi"', "x\ny"), ("", 7)])
        self.assertEqual(r.to_csv(), 'a,"b,c"\n,true\n2.5,false\n"say ""hi""","x\ny"\n,7\n')

    def test_query_to_csv(self):
        out = query("SELECT name, score, age > 30 AS old FROM people WHERE city = 'Paris'", tables()).to_csv()
        self.assertEqual(out, "name,score,old\nBob,7,\nDee,,true\n")
        out = query("SELECT AVG(age) AS a FROM people WHERE city = 'Rome'", tables()).to_csv()
        self.assertEqual(out, "a\n25.0\n")
