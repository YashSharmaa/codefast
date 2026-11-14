import contextlib
import io
import os
import subprocess
import sys
import tempfile
import unittest

from csvq.cli import main

from .support import PEOPLE_CSV

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def call(*argv):
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        code = main(list(argv))
    return code, out.getvalue(), err.getvalue()


class CliTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.people = os.path.join(self.dir.name, "people.csv")
        with open(self.people, "w", encoding="utf-8") as f:
            f.write(PEOPLE_CSV)
        self.cities = os.path.join(self.dir.name, "cities.csv")
        with open(self.cities, "w", encoding="utf-8") as f:
            f.write("city,country\nLondon,UK\nParis,France\n")

    def tearDown(self):
        self.dir.cleanup()

    def test_success(self):
        code, out, err = call("SELECT city, COUNT(*) AS n FROM p GROUP BY city ORDER BY n DESC, city",
                              "--table", f"p={self.people}", "--table", f"c={self.cities}")
        self.assertEqual((code, out, err), (0, "city,n\nLondon,2\nParis,2\nRome,1\n", ""))
        code, out, _ = call("SELECT country FROM c WHERE city = 'Paris'", "--table", f"p={self.people}",
                            "--table", f"c={self.cities}")
        self.assertEqual((code, out), (0, "country\nFrance\n"))

    def test_query_errors_exit_1(self):
        for argv in [("SELECT nope FROM p", "--table", f"p={self.people}"),
                     ("SELECT name FROM", "--table", f"p={self.people}"),
                     ("SELECT * FROM p", "--table", f"p={os.path.join(self.dir.name, 'missing.csv')}")]:
            with self.subTest(argv=argv):
                code, out, err = call(*argv)
                self.assertEqual((code, out), (1, ""))
                self.assertTrue(err.startswith("error: "))
                self.assertEqual(len(err.strip().splitlines()), 1)

    def test_usage_errors_exit_2(self):
        for argv in [(), ("SELECT * FROM p",), ("SELECT * FROM p", "--table", self.people)]:
            with self.subTest(argv=argv):
                code, out, err = call(*argv)
                self.assertEqual(code, 2)
                self.assertNotEqual(err, "")

    def test_python_dash_m(self):
        p = subprocess.run([sys.executable, "-m", "csvq", "SELECT name FROM p WHERE age = 25", "--table",
                            f"p={self.people}"], cwd=ROOT, capture_output=True, text=True, timeout=30)
        self.assertEqual((p.returncode, p.stdout), (0, "name\nCy\nEve\n"))
