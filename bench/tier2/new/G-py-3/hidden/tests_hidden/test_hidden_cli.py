import contextlib
import io
import os
import subprocess
import sys
import unittest

from dimcalc.cli import main

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def call(*argv):
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        code = main(list(argv))
    return code, out.getvalue(), err.getvalue()


class CliTests(unittest.TestCase):
    def test_convert(self):
        self.assertEqual(call("convert", "100", "degC", "degF"), (0, "212\n", ""))
        self.assertEqual(call("convert", "1", "km/h", "m/s")[:2], (0, "0.277778\n"))
        self.assertEqual(call("convert", "-3.5", "kg", "g")[:2], (0, "-3500\n"))

    def test_base_dim_check(self):
        self.assertEqual(call("base", "3 kN")[:2], (0, "3000 m*kg/s^2\n"))
        self.assertEqual(call("dim", "J/kg/K")[:2], (0, "m^2/s^2*K\n"))
        self.assertEqual(call("dim", "Hz")[:2], (0, "1/s\n"))
        self.assertEqual(call("check", "N*m", "J")[:2], (0, "compatible\n"))
        self.assertEqual(call("check", "W", "J")[:2], (0, "incompatible\n"))

    def test_split_and_compare(self):
        self.assertEqual(call("split", "70 in", "ft,in")[:2], (0, "5 ft 10 in\n"))
        self.assertEqual(call("split", "3725 s", "h,min,s")[:2], (0, "1 h 2 min 5 s\n"))
        self.assertEqual(call("compare", "1 ft", "12 in")[:2], (0, "=\n"))
        self.assertEqual(call("compare", "1 mi", "1 km")[:2], (0, ">\n"))
        self.assertEqual(call("compare", "0 degF", "0 degC")[:2], (0, "<\n"))

    def test_errors(self):
        for argv in [("convert", "1", "m", "s"), ("dim", "furlong"), ("base", "km"), ("compare", "1 m", "1 s"),
                     ("split", "1 m", "s")]:
            with self.subTest(argv=argv):
                code, out, err = call(*argv)
                self.assertEqual((code, out), (1, ""))
                self.assertTrue(err.startswith("error: "))
                self.assertEqual(len(err.strip().splitlines()), 1)

    def test_usage_errors(self):
        for argv in [(), ("frobnicate",), ("convert", "1", "m"), ("dim",), ("check", "m"),
                     ("convert", "abc", "m", "km"), ("dim", "m", "s")]:
            with self.subTest(argv=argv):
                code, out, err = call(*argv)
                self.assertEqual((code, out), (2, ""))
                self.assertNotEqual(err, "")

    def test_python_dash_m(self):
        p = subprocess.run([sys.executable, "-m", "dimcalc", "convert", "1", "mi", "ft"],
                           cwd=ROOT, capture_output=True, text=True, timeout=30)
        self.assertEqual((p.returncode, p.stdout), (0, "5280\n"))
