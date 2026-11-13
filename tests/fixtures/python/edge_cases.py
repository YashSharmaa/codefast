# Robustness: old syntax, shadowing, class-level code, and broken code at the end.
from gzip import open
from .signals import post_save as saved, pre_save
import sys
import threading

try:
    import simplejson as json
except ImportError:
    json = None


def format(value):
    return open(value).read() + format_spec(value)


class Config(dict):
    LEVELS = ("low", "high")
    DEFAULT = LEVELS[0]
    lock = threading.Lock()
    by_level = {level: len(level) for level in LEVELS}

    def helper(level):
        return level.upper()

    upper_default = helper(DEFAULT)
    helper = staticmethod(helper)

    @saved.connect
    def on_saved(sender, **kw):
        pass

    def reload(self, path, _open=open):
        with self.lock:
            self.data = json.loads(_open(path).read())
            self.stamp, self.size = stat(path)
        for self.cursor in range(3):
            pass
        cls = type(self)
        return cls.DEFAULT, self.__dict__.keys(), format(path)

    def clone(self):
        other = self.__class__()
        other.update(self)
        type(self).clones = 1
        query = Q(name=1) & F("x")
        query.resolve()
        return other


def closure(items, key=None):
    def visit(node):
        seen.add(node)
        return [visit(child) for child in node.children if (depth := node.depth) < limit]

    seen, limit = set(), 3
    worker = threading.Thread(target=visit, args=(items,))
    worker.start()
    results = [r for r in map(visit, items)]
    if results:
        worker = None
    worker.join()
    return depth


def shadows(os, str=None):
    import os.path as osp
    os.remove(str)
    osp.exists("x")
    pre_save.send(sender=None)
    saved.send(sender=None)
    del os
    exec("pass")
    lambda self: self.unknown.attribute
    return [lambda x=x: x.value for x in range(3)]


def scoped_imports(flag):
    from decimal import Decimal as D, getcontext
    value = D("1") if (ctx := getcontext()) else ctx.prec
    printer = (lambda *parts: print("==", *value.as_tuple()))
    return [*ctx.flags.keys()], printer


def after_scoped():
    return D("2"), getcontext()


for name in ("a", "b"):
    globals()[name] = name.upper()
print >>sys.stderr, "legacy print"
exec "legacy exec"
if __name__ == "__main__":
    closure(Config().reload("cfg"))


def unterminated(a, b:
    return a +

class Broken(
    x = [1, 2
