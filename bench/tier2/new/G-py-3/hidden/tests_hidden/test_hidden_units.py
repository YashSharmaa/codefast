import unittest

from dimcalc import DIMENSIONLESS, Dimension, DimcalcError, DimensionError, Unit, UnitError, lookup, parse_unit

M = Dimension((1, 0, 0, 0, 0, 0))
KG = Dimension((0, 1, 0, 0, 0, 0))
S = Dimension((0, 0, 1, 0, 0, 0))
K = Dimension((0, 0, 0, 0, 1, 0))


class DimensionTests(unittest.TestCase):
    def test_str(self):
        self.assertEqual(str(Dimension((1, 1, -2, 0, 0, 0))), "m*kg/s^2")
        self.assertEqual(str(Dimension((0, 0, -1, 0, 0, 0))), "1/s")
        self.assertEqual(str(Dimension((2, 0, -2, 0, -1, 0))), "m^2/s^2*K")
        self.assertEqual(str(Dimension((0, 0, -2, 0, 0, 0))), "1/s^2")
        self.assertEqual(str(Dimension((0, 0, 0, 1, 0, 3))), "A*mol^3")
        self.assertEqual(str(DIMENSIONLESS), "1")

    def test_arithmetic_and_equality(self):
        self.assertEqual(M * KG / S ** 2, Dimension((1, 1, -2, 0, 0, 0)))
        self.assertEqual((M / M), DIMENSIONLESS)
        self.assertEqual(M ** 3, Dimension((3, 0, 0, 0, 0, 0)))
        self.assertEqual(len({M * S, S * M, Dimension((1, 0, 1, 0, 0, 0))}), 1)
        self.assertNotEqual(M, S)
        self.assertEqual(DIMENSIONLESS.exponents, (0, 0, 0, 0, 0, 0))

    def test_error_hierarchy(self):
        self.assertTrue(issubclass(UnitError, DimcalcError))
        self.assertTrue(issubclass(DimensionError, DimcalcError))
        self.assertFalse(issubclass(UnitError, DimensionError))


class LookupTests(unittest.TestCase):
    def test_exact_names(self):
        self.assertEqual((lookup("min").factor, lookup("min").dimension), (60, S))
        self.assertEqual((lookup("m").factor, lookup("m").dimension), (1, M))
        self.assertEqual(lookup("mi").factor, 1609.344)
        self.assertEqual(lookup("day").factor, 86400)
        self.assertEqual(lookup("g").factor, 0.001)
        self.assertEqual(lookup("g").dimension, KG)
        self.assertEqual(lookup("bar").factor, 100000)
        self.assertEqual(lookup("Pa").dimension, Dimension((-1, 1, -2, 0, 0, 0)))
        self.assertEqual(lookup("V").dimension, Dimension((2, 1, -3, -1, 0, 0)))
        self.assertEqual(lookup("C").dimension, Dimension((0, 0, 1, 1, 0, 0)))
        self.assertEqual(lookup("L").dimension, M ** 3)

    def test_prefixes(self):
        cases = {"kg": (1.0, KG), "mm": (0.001, M), "km": (1000.0, M), "us": (1e-6, S),
                 "GHz": (1e9, DIMENSIONLESS / S), "kPa": (1000.0, Dimension((-1, 1, -2, 0, 0, 0))),
                 "mmol": (0.001, Dimension((0, 0, 0, 0, 0, 1))), "cL": (1e-5, M ** 3),
                 "Mbar": (1e11, Dimension((-1, 1, -2, 0, 0, 0))), "nK": (1e-9, K)}
        for name, (factor, dim) in cases.items():
            with self.subTest(name=name):
                u = lookup(name)
                self.assertAlmostEqual(u.factor / factor, 1.0, places=12)
                self.assertEqual(u.dimension, dim)
                self.assertEqual(u.name, name)
                self.assertEqual(u.offset, 0.0)

    def test_lookup_errors(self):
        for name in ["kmin", "Kg", "foo", "", "kdegC", "mft", "x", "kday", "KG", "meter"]:
            with self.subTest(name=name):
                with self.assertRaises(UnitError):
                    lookup(name)

    def test_temperature_units_and_frozen(self):
        c, f = lookup("degC"), lookup("degF")
        self.assertEqual((c.factor, c.offset, c.dimension), (1, 273.15, K))
        self.assertAlmostEqual(f.factor, 5 / 9)
        self.assertAlmostEqual(f.offset, 459.67 * 5 / 9)
        self.assertIsInstance(c, Unit)
        with self.assertRaises(AttributeError):
            c.factor = 2.0


class ParseUnitTests(unittest.TestCase):
    def test_compound_expressions(self):
        u = parse_unit("km/h")
        self.assertAlmostEqual(u.factor, 1000 / 3600)
        self.assertEqual(u.dimension, M / S)
        self.assertEqual(parse_unit("kg*m/s^2").dimension, lookup("N").dimension)
        self.assertEqual(parse_unit("1/s").dimension, lookup("Hz").dimension)
        self.assertEqual(parse_unit("m^-1").dimension, Dimension((-1, 0, 0, 0, 0, 0)))
        self.assertEqual(parse_unit("m^0").dimension, DIMENSIONLESS)
        self.assertAlmostEqual(parse_unit("ft^2").factor, 0.3048 ** 2)

    def test_left_to_right_grouping(self):
        a, b = parse_unit("J/kg/K"), parse_unit("J/(kg*K)")
        self.assertEqual(a.dimension, b.dimension)
        self.assertEqual(a.dimension, Dimension((2, 0, -2, 0, -1, 0)))
        self.assertAlmostEqual(parse_unit("km/h*h").factor, 1000)
        self.assertAlmostEqual(parse_unit("g/mm/s").factor, 1.0)
        self.assertEqual(parse_unit("(m/s)^2").dimension, Dimension((2, 0, -2, 0, 0, 0)))

    def test_name_and_spaces(self):
        u = parse_unit("  kg * m / s ^ 2 ")
        self.assertEqual(u.name, "kg * m / s ^ 2")
        self.assertEqual(u.dimension, Dimension((1, 1, -2, 0, 0, 0)))
        self.assertEqual(u.offset, 0.0)
        t = parse_unit(" degF ")
        self.assertEqual(t.name, "degF")
        self.assertAlmostEqual(t.offset, 459.67 * 5 / 9)

    def test_parse_errors(self):
        for text in ["", "  ", "m*", "*m", "m^", "m^x", "m^2.5", "(m", "m)", "2*m", "m**2", "m//s",
                     "kg m", "degC/s", "m*degF", "degC^2", "m2", "furlong", "m/(s", "m+s"]:
            with self.subTest(text=text):
                with self.assertRaises(UnitError):
                    parse_unit(text)
