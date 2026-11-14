import unittest

from dimcalc import (DIMENSIONLESS, Dimension, DimensionError, Quantity, UnitError, convert,
                     parse_quantity, split)


class ConversionTests(unittest.TestCase):
    def test_linear_conversions(self):
        cases = [(1, "km/h", "m/s", 1 / 3.6), (1, "mi", "km", 1.609344), (2, "lb", "kg", 0.90718474),
                 (1, "h", "s", 3600), (1.5, "L", "m^3", 0.0015), (2, "bar", "kPa", 200),
                 (1, "kW*h", "MJ", 3.6), (3, "ft", "in", 36), (1, "N*m", "J", 1), (5, "g/cm^3", "kg/L", 5)]
        for value, a, b, want in cases:
            with self.subTest(a=a, b=b):
                self.assertAlmostEqual(convert(value, a, b) / want, 1.0, places=9)

    def test_temperatures(self):
        self.assertAlmostEqual(convert(100, "degC", "degF"), 212)
        self.assertAlmostEqual(convert(32, "degF", "degC"), 0, places=9)
        self.assertAlmostEqual(convert(-40, "degC", "degF"), -40)
        self.assertAlmostEqual(convert(0, "K", "degC"), -273.15)
        self.assertAlmostEqual(convert(300, "K", "degF"), 80.33)
        self.assertAlmostEqual(convert(20, "degC", "mK"), 293150)

    def test_conversion_errors(self):
        with self.assertRaises(DimensionError):
            convert(1, "m", "s")
        with self.assertRaises(DimensionError):
            Quantity(1, "J").to("W")
        with self.assertRaises(UnitError):
            Quantity(1, "m").to("parsec")
        with self.assertRaises(UnitError):
            Quantity(1, "furlong")

    def test_attributes_and_in_base(self):
        q = Quantity(3, "  km/h ")
        self.assertEqual(q.unit, "km/h")
        self.assertEqual(q.value, 3)
        self.assertEqual(q.dimension, Dimension((1, 0, -1, 0, 0, 0)))
        b = parse_quantity("3 kN").in_base()
        self.assertEqual((b.value, b.unit), (3000, "m*kg/s^2"))
        self.assertEqual(Quantity(2, "h").in_base().unit, "s")
        self.assertAlmostEqual(Quantity(25, "degC").in_base().value, 298.15)
        self.assertEqual(Quantity(25, "degC").in_base().unit, "K")
        self.assertEqual(Quantity(5, "1").in_base().unit, "1")
        t = Quantity(2, "km").to("m")
        self.assertEqual((t.value, t.unit), (2000, "m"))


class ArithmeticTests(unittest.TestCase):
    def test_add_and_subtract_use_left_unit(self):
        q = Quantity(1, "km") + Quantity(500, "m")
        self.assertEqual(q.unit, "km")
        self.assertAlmostEqual(q.value, 1.5)
        q = Quantity(1, "m") - Quantity(50, "cm")
        self.assertEqual(q.unit, "m")
        self.assertAlmostEqual(q.value, 0.5)
        with self.assertRaises(DimensionError):
            Quantity(1, "m") + Quantity(1, "s")

    def test_multiply_and_divide_quantities(self):
        q = Quantity(2, "m") * Quantity(3, "s")
        self.assertEqual((q.value, q.unit), (6, "(m)*(s)"))
        self.assertEqual(q.dimension, Dimension((1, 0, 1, 0, 0, 0)))
        speed = Quantity(2, "km") / Quantity(30, "min")
        self.assertEqual(speed.unit, "(km)/(min)")
        self.assertAlmostEqual(speed.to("km/h").value, 4)
        area = Quantity(2, "m/s") * Quantity(3, "s")
        self.assertEqual(area.dimension, Dimension((1, 0, 0, 0, 0, 0)))

    def test_scalars(self):
        self.assertEqual(((Quantity(3, "kg") * 2).value, (Quantity(3, "kg") * 2).unit), (6, "kg"))
        self.assertEqual(((2 * Quantity(3, "kg")).value, (2 * Quantity(3, "kg")).unit), (6, "kg"))
        q = Quantity(3, "kg") / 4
        self.assertEqual((q.value, q.unit), (0.75, "kg"))
        f = 10 / Quantity(4, "s")
        self.assertEqual((f.value, f.unit), (2.5, "1/(s)"))
        self.assertEqual(f.dimension, lookup_hz())

    def test_affine_arithmetic_rejected(self):
        c = Quantity(20, "degC")
        for op in [lambda: c + Quantity(1, "degC"), lambda: Quantity(1, "K") + c, lambda: c * 2,
                   lambda: 2 * c, lambda: c / Quantity(1, "s"), lambda: 1 / c, lambda: c - c,
                   lambda: Quantity(1, "m") * Quantity(1, "degF")]:
            with self.assertRaises(DimensionError):
                op()

    def test_str(self):
        self.assertEqual(str(Quantity(1 / 3, "m")), "0.333333 m")
        self.assertEqual(str(Quantity(1234567, "g")), "1.23457e+06 g")
        self.assertEqual(str(Quantity(2, "m") * Quantity(3, "s")), "6 (m)*(s)")

    def test_comparisons(self):
        self.assertTrue(Quantity(1, "ft") == Quantity(12, "in"))
        self.assertFalse(Quantity(1, "ft") != Quantity(12, "in"))
        self.assertFalse(Quantity(1, "ft") < Quantity(12, "in"))
        self.assertTrue(Quantity(1, "ft") <= Quantity(12, "in"))
        self.assertTrue(Quantity(1, "km") > Quantity(999, "m"))
        self.assertTrue(Quantity(1, "kg") >= Quantity(1, "lb"))
        self.assertTrue(Quantity(0, "degC") == Quantity(32, "degF"))
        self.assertTrue(Quantity(10, "degC") > Quantity(40, "degF"))
        with self.assertRaises(DimensionError):
            Quantity(1, "m") < Quantity(1, "s")
        with self.assertRaises(DimensionError):
            Quantity(1, "m") == Quantity(1, "s")


def lookup_hz():
    return Dimension((0, 0, -1, 0, 0, 0))


class ParseQuantityTests(unittest.TestCase):
    def test_forms(self):
        cases = {"3.5 km/h": (3.5, "km/h"), "-2e3 m": (-2000, "m"), "  10   ft": (10, "ft"),
                 "7kg": (7, "kg"), "+1.5E-3 s": (0.0015, "s"), "9.81 m/s^2": (9.81, "m/s^2")}
        for text, (value, unit) in cases.items():
            with self.subTest(text=text):
                q = parse_quantity(text)
                self.assertAlmostEqual(q.value, value)
                self.assertEqual(q.unit, unit)
        q = parse_quantity("42")
        self.assertEqual((q.value, q.unit, q.dimension), (42, "1", DIMENSIONLESS))

    def test_errors(self):
        for text in ["km", "", ".5 m", "3 furlong", "abc 3", "3 m*"]:
            with self.subTest(text=text):
                with self.assertRaises(UnitError):
                    parse_quantity(text)


class SplitTests(unittest.TestCase):
    def values(self, parts):
        return [(round(p.value, 6), p.unit) for p in parts]

    def test_split(self):
        self.assertEqual(self.values(split(parse_quantity("70 in"), ["ft", "in"])), [(5, "ft"), (10, "in")])
        self.assertEqual(self.values(split(Quantity(1.75, "h"), ["h", "min"])), [(1, "h"), (45, "min")])
        self.assertEqual(self.values(split(Quantity(3725, "s"), ["h", "min", "s"])), [(1, "h"), (2, "min"), (5, "s")])
        self.assertEqual(self.values(split(Quantity(0.3, "h"), ["h", "min"])), [(0, "h"), (18, "min")])
        parts = split(Quantity(2.5, "kg"), ["lb"])
        self.assertAlmostEqual(parts[0].value, 2.5 / 0.45359237)

    def test_integer_parts_use_tolerance(self):
        parts = split(Quantity(0.57, "m"), ["cm", "mm"])
        self.assertEqual(parts[0].value, 57)
        self.assertAlmostEqual(parts[1].value, 0, places=6)

    def test_negative(self):
        self.assertEqual(self.values(split(Quantity(-90, "min"), ["h", "min"])), [(-1, "h"), (-30, "min")])

    def test_errors(self):
        with self.assertRaises(DimensionError):
            split(Quantity(1, "m"), ["ft", "s"])
        with self.assertRaises(UnitError):
            split(Quantity(1, "m"), [])
        with self.assertRaises(DimensionError):
            split(Quantity(300, "K"), ["degC"])
        with self.assertRaises(DimensionError):
            split(Quantity(30, "degC"), ["K"])
