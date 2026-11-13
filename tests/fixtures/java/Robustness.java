// Unusual and broken input: the extractor must keep going and record what it can.
module com.acme.shop {
    requires java.base;
    exports com.acme.shop;
}

import java.util.List

String banner = "a compact source file: fields and methods of an implicit class";

void main() {
    greet(banner);
    banner = null;
}

class Broken {
    int before;

    void incomplete(Foo foo) {
        int a = ;
        Foo f = new Foo(a,);
        f.go(;
        if (a > ) {
            stop(f, );
        }
        foo.
    }

    void fine(Broken other) {
        other.before = 1;
        fine(this);
    }

    int after = compute(before);
}

enum Empty {}

interface { void unnamed(); }

class Trailing extends Broken {
    static Trailing of() {
        return new Trailing();
    }

    void last() {
        before++;
        x.
