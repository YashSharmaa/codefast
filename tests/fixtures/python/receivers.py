"""Receiver parameters under unusual names, and what `*args` / `**kwargs` are."""


class Registry(type):
    # `__new__` takes the class first whatever the parameter is called.
    def __new__(mcls, name, bases, ns):
        inst = super().__new__(mcls, name, bases, ns)
        mcls.known.add(inst)
        return inst

    # So does a classmethod: `klass` is the class, not an argument.
    @classmethod
    def build(klass, size, *parts, **options):
        made = klass(size)
        made.run()
        klass.helper()
        return made

    @classmethod
    def helper(typ):
        return typ

    # A plain method with an unusual first parameter is left alone: it may be a helper
    # that the class body calls like a function.
    def odd(this, other):
        this.run()

    def run(self, *args, **kwargs):
        # Unpacked parameters are a tuple and a dict ...
        args.count(1)
        kwargs.items()

    def rebound(self, *labels, **options):
        # ... until the name is bound to something else.
        labels = set(labels)
        labels.pop()
        options = Options(options)
        options.validate()


def free(klass, self_like=None, *rest):
    rest.index(1)
    return klass()
