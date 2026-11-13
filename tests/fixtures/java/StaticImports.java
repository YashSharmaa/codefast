package com.acme.shop;

import module java.base;
import static com.acme.util.Checks.require;
import static com.acme.util.Limits.MAX;
import static com.acme.util.Registry.INSTANCE;
import static com.acme.model.Outer.Inner.deep;
import static com.acme.util.Wild.*;
import static com.acme.a.Fmt.format;
import static com.acme.b.Fmt.format;

// A name bound by `import static a.b.C.name` is a member of `C`: its references carry the
// qualifier a fully qualified `a.b.C.name` would have.
class StaticImports extends Base {
    int limit = MAX;
    Object held = require(MAX);

    Object plain(Object o) {
        Object max = require(o);
        INSTANCE.register(max);
        require(o).toString();
        deep();
        format("two static imports bind this name: the class qualifies it");
        wild(WILD);
        return MAX;
    }

    // A parameter or local hides the imported constant, never the imported method.
    Object locals(int MAX, Object INSTANCE) {
        INSTANCE.hashCode();
        return require(MAX);
    }

    // A method the class declares itself hides the static import in the whole body.
    static class Hiding {
        Object require(Object o) { return o; }
        Object use(Object o) { return require(o); }
        class Deeper { Object use(Object o) { return require(MAX); } }
    }

    // A field of the class hides the imported constant.
    static class HidingField {
        int MAX = 1;
        int use() { return MAX + require(1).hashCode(); }
    }

    Runnable anonymous() {
        return new Runnable() {
            Object require(Object o) { return o; }
            public void run() { require(MAX); deep(); }
        };
    }

    interface Api {
        default Object checked(Object o) { return require(o); }
    }

    enum Level {
        LOW { Object pick() { return require(MAX); } };
        Object pick() { return null; }
    }
}
