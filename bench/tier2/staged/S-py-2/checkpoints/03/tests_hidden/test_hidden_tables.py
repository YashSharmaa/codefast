import unittest

from csvq import CsvqError, QueryError, QuerySyntaxError, Table, query

from .support import tables


class TableTests(unittest.TestCase):
    def test_field_conversion(self):
        t = Table.from_csv('a,b,c,d,e,f\n007,-3,-0.5,,"x, y",1e5\n3.,.5, 5,abc,"",-\n')
        self.assertEqual(t.columns, ["a", "b", "c", "d", "e", "f"])
        self.assertEqual(t.rows[0], (7, -3, -0.5, None, "x, y", "1e5"))
        self.assertEqual(t.rows[1], ("3.", ".5", " 5", "abc", None, "-"))
        self.assertIsInstance(t.rows[0][0], int)
        self.assertIsInstance(t.rows[0][2], float)

    def test_blank_lines_and_errors(self):
        t = Table.from_csv("a,b\n\n1,2\n\n3,4\n")
        self.assertEqual(t.rows, [(1, 2), (3, 4)])
        for text in ["", "a,a\n1,2\n", "a,b\n1,2,3\n", "a,b\n1\n"]:
            with self.subTest(text=text):
                with self.assertRaises(QueryError):
                    Table.from_csv(text)

    def test_error_hierarchy(self):
        self.assertTrue(issubclass(QuerySyntaxError, CsvqError))
        self.assertTrue(issubclass(QueryError, CsvqError))
        self.assertFalse(issubclass(QuerySyntaxError, QueryError))
        self.assertFalse(issubclass(QueryError, QuerySyntaxError))


class SyntaxTests(unittest.TestCase):
    def test_syntax_errors(self):
        bad = ["SELECT", "SELECT name people", "SELECT name FROM", "SELECT name FROM people WHERE",
               "SELECT name FROM people LIMIT -1", "SELECT name FROM people LIMIT 1.5",
               "SELECT 'abc FROM people", "SELECT name FROM people;", "SELECT * , name FROM people",
               "SELECT name FROM people WHERE age = 1 = 2", "SELECT count FROM people",
               "SELECT name FROM people ORDER name", "SELECT # FROM people", "SELECT (age FROM people",
               "SELECT name FROM people GROUP BY", 'SELECT "name FROM people', "FROM people SELECT name"]
        for sql in bad:
            with self.subTest(sql=sql):
                with self.assertRaises(QuerySyntaxError):
                    query(sql, tables())

    def test_keywords_case_insensitive_names_case_sensitive(self):
        r = query("select name from people where age > 30 order by name desc limit 5", tables())
        self.assertEqual(r.rows, [("Dee",), ("Ada",)])
        with self.assertRaises(QueryError):
            query("SELECT Name FROM people", tables())
        with self.assertRaises(QueryError):
            query("SELECT name FROM People", tables())

    def test_quoted_names_and_strings(self):
        t = {"t": Table(["first name", 'say "hi"'], [("Ann", "it's"), ("Ben", "no")])}
        r = query('SELECT "first name" AS n, "say ""hi""" FROM t WHERE "say ""hi""" = \'it\'\'s\'', t)
        self.assertEqual(r.columns, ["n", 'say "hi"'])
        self.assertEqual(r.rows, [("Ann", "it's")])
