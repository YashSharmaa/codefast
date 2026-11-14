import contextlib
import io
import os
import tempfile
import unittest

from csvq import QueryError, Result, Table, query
from csvq.cli import main

ZIPS_CSV = 'zip,id,note\n"02134","007",""\n'


class QuotedFieldTests(unittest.TestCase):
    def test_example_from_the_report(self):
        t = {"t": Table.from_csv(ZIPS_CSV)}
        r = query("SELECT * FROM t", t)
        self.assertEqual(r.rows, [("02134", "007", "")])
        self.assertEqual(r.to_csv(), 'zip,id,note\n"02134","007",""\n')

    def test_unquoted_fields_still_converted(self):
        t = Table.from_csv('a,b,c,d\n"1",1,,""\n"x",x,-2.5,"-2.5"\n')
        self.assertEqual(t.rows, [("1", 1, None, ""), ("x", "x", -2.5, "-2.5")])
        self.assertIs(type(t.rows[0][1]), int)
        self.assertIs(type(t.rows[1][2]), float)

    def test_quoted_text_is_kept_exactly(self):
        t = Table.from_csv('a,b,c,d\n" 5","a,b","say ""7""","x\ny"\n"""","-","1e5","NULL"\n')
        self.assertEqual(t.rows, [(" 5", "a,b", 'say "7"', "x\ny"), ('"', "-", "1e5", "NULL")])

    def test_line_endings_inside_and_outside_quotes(self):
        t = Table.from_csv('a,b\r\n"x\r\ny","2"\r\n\r\n3,"4"\r\n')
        self.assertEqual(t.rows, [("x\r\ny", "2"), (3, "4")])

    def test_header_is_never_converted(self):
        t = Table.from_csv('"id",1,"007"\n1,2,3\n')
        self.assertEqual(t.columns, ["id", "1", "007"])
        self.assertEqual(t.rows, [(1, 2, 3)])

    def test_queries_see_strings(self):
        t = {"t": Table.from_csv(ZIPS_CSV)}
        self.assertEqual(query("SELECT id FROM t WHERE zip = '02134'", t).rows, [("007",)])
        self.assertEqual(query("SELECT LENGTH(note), note IS NULL, COUNT(note) FROM t GROUP BY note", t).rows,
                         [(0, False, 1)])
        for sql in ["SELECT id FROM t WHERE id = 7", "SELECT id + 1 FROM t", "SELECT SUM(zip) FROM t"]:
            with self.subTest(sql=sql):
                with self.assertRaises(QueryError):
                    query(sql, t)

    def test_to_csv_quotes_strings_that_look_converted(self):
        r = Result(["a", "b", "c", "d"], [("007", 7, "", None), ("1.5", 1.5, "-2", "x"), ("-0", -0.5, "12.50", "1.5.1")])
        self.assertEqual(r.to_csv(), 'a,b,c,d\n"007",7,"",\n"1.5",1.5,"-2",x\n"-0",-0.5,"12.50",1.5.1\n')

    def test_to_csv_leaves_other_strings_alone(self):
        r = Result(["s"], [("1e5",), ("1.",), (" 7",), ("+7",), ("true",), (".5",), ("7 ",)])
        self.assertEqual(r.to_csv(), "s\n1e5\n1.\n 7\n+7\ntrue\n.5\n7 \n")
        r = Result(["b", "n"], [(True, None), (False, 0)])
        self.assertEqual(r.to_csv(), "b,n\ntrue,\nfalse,0\n")

    def test_header_follows_the_same_rules(self):
        self.assertEqual(Result(["1", "", "x", "2.0", "a b"], []).to_csv(), '"1","",x,"2.0",a b\n')

    def test_output_reads_back_the_same(self):
        t = {"t": Table(["s", "n", "e"], [("007", 7, ""), ("", None, "a,b"), ("-1.25", -1.25, 'q"')])}
        r = query("SELECT * FROM t", t)
        back = Table.from_csv(r.to_csv())
        self.assertEqual((back.columns, back.rows), (r.columns, r.rows))

    def test_reading_rules_otherwise_unchanged(self):
        t = Table.from_csv('a,b\n\n"1",2\n\n')
        self.assertEqual(t.rows, [("1", 2)])
        for text in ['a,b\n"1",2,3\n', '"a",a\n1,2\n', "", "\n\n", 'a,b\n"x"\n']:
            with self.subTest(text=text):
                with self.assertRaises(QueryError):
                    Table.from_csv(text)

    def test_functions_on_quoted_values(self):
        t = {"t": Table.from_csv('code,n\n"0042",42\n')}
        self.assertEqual(query("SELECT code, CONCAT(code, '!'), COALESCE(NULLIF(code, '0042'), 'gone'), CONCAT(n) FROM t",
                               t).rows, [("0042", "0042!", "gone", "42")])
        self.assertEqual(query("SELECT CONCAT(code), CONCAT(n), n FROM t", t).to_csv(), 'col1,col2,n\n"0042","42",42\n')

    def test_cli_keeps_quoted_text(self):
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "zips.csv")
            with open(path, "w", encoding="utf-8") as f:
                f.write(ZIPS_CSV + '"10001",8,x\n')
            out, err = io.StringIO(), io.StringIO()
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                code = main(["SELECT zip, id, note FROM z ORDER BY zip DESC", "--table", f"z={path}"])
        self.assertEqual((code, out.getvalue(), err.getvalue()), (0, 'zip,id,note\n"10001",8,x\n"02134","007",""\n', ""))
