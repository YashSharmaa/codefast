"""Definitions: every kind of symbol the extractor emits."""
import unittest
from abc import ABC, abstractmethod
from dataclasses import dataclass
from functools import cached_property
from typing import Callable, ClassVar, Optional, Protocol, TypeAlias, overload

__all__ = ["Shape", "area_of", "MAX_SIDES"]

MAX_SIDES = 12
DEFAULT_NAME: str = "shape"
registry = {}
counter: int
origin, unit = (0, 0), (1, 1)
first = second = None
MAX_SIDES = 16
Vector: TypeAlias = "tuple[float, float]"
Number = Decimal
type Pair[T] = tuple[T, T]

square = lambda side, scale=1: side * side * scale


def area_of(shape: "Shape", *, precise: bool = False) -> float:
    def clamp(value, low=0.0, high=1.0):
        return max(low, min(value, high))

    class Result:
        ok = True

    halve = lambda x: x / 2
    return clamp(halve(shape.area()))


async def fetch(url, *args, timeout=10, **options) -> Optional["Shape"]:
    return await load(url, *args, timeout=timeout, **options)


def variadic(self, a, b=1, /, c=2, *rest): ...


class Meta(type):
    pass


class Shape(ABC, metaclass=Meta):
    """A shape."""

    sides: ClassVar[int] = 0
    name = DEFAULT_NAME
    on_change: Optional[Callable[[int], None]] = None
    tags, labels = [], {}
    ident = property(lambda self: id(self))

    def __init__(self, name: str, parent: "Shape" = None, *children, origin=origin):
        self.name = name
        self.parent = parent
        self.bounds: Rect = None
        self.cache = Cache(children)
        self.dirty = None
        self.listener = lambda event: None
        self.origin, self.extent = origin, None
        self.dirty = Flag()

    @abstractmethod
    def area(self) -> float: ...

    @property
    def label(self) -> str:
        return self.name.title()

    @label.setter
    def label(self, value):
        self.name = value
        self.renamed = True

    @cached_property
    def perimeter(self):
        return compute(self)

    @staticmethod
    def unit(scale, offset=0) -> "Shape":
        return Square(scale)

    @classmethod
    def named(cls, name) -> "Shape":
        shape = cls(name)
        cls.last_named = shape
        return shape

    async def render(self, canvas: Canvas) -> None:
        await canvas.draw(self)

    def describe(self):
        raise NotImplementedError("subclasses describe themselves")

    class Style:
        width = 1

        def scaled(self, factor):
            return self.width * factor


class Drawable(Protocol):
    visible: bool

    def draw(self, canvas: "Canvas") -> None: ...

    def hide(self) -> None:
        """Hide it."""

    def bounds(self):
        pass


@dataclass(frozen=True)
class Rect:
    left: float
    top: float = 0.0
    corners: list["Point"] = None


@overload
def scale(x: int) -> int: ...
@overload
def scale(x: float) -> float: ...
def scale(x):
    return x * 2


@registry.register("square", priority=very_long_priority_constant_name, description="A long decorator call that would bury the signature")
class Square(Shape):
    def area(self):
        return self.side**2


class TestShape:
    def setup_method(self):
        self.shape = Square(2)

    def test_area(self):
        assert self.shape.area() == 4


def test_scale():
    assert scale(2) == 4


class GeometryTests(unittest.TestCase):
    def helper(self):
        return 1


if DEBUG:
    def debug_only():
        return registry
else:
    debug_only = None
