package com.acme.shop;

import com.acme.io.Channel;
import com.acme.repo.Repo;
import com.acme.repo.*;
import static com.acme.util.Checks.require;
import static com.acme.util.Limits.*;

/** Contract of 2026-11-13: receiver type paths, module-qualified names, imports. */
public class Contract extends com.acme.base.Service implements java.io.Closeable, Outer.Listener {
    private final Repo repo;
    private Service service;
    java.util.List<String> names;
    java.util.Map.Entry<String, Order> entry;
    Outer.Inner inner;
    com.acme.model.Outer.Inner far;
    Outer.Inner.Deep[] deep;

    Contract(Repo repo) {
        super(repo);
        this.repo = repo;
    }

    java.util.Optional<Order> find(long id) {
        return java.util.Optional.empty();
    }

    Outer.Inner make() {
        return new Outer.Inner();
    }

    void chains(long id, Order x) {
        repo.find(id).getName();
        this.service.client().send();
        service.client().send();
        Foo.of(x).build();
        Foo.of(x).build().run();
        find(id).get();
        this.find(id).orElse(null);
        super.open().close();
        make().go();
        new Order(x).line(0).price();
        ((Order) x).total().cents();
        com.acme.util.Ids.next().toString();
        java.util.Objects.requireNonNull(x).cancel();
        Outer.Inner.create().go();
        x.lines[0].price().cents();
        repo.find(id).owner.notify(x);
        a().b().c().d().e().f();
    }

    void typed(java.util.List<Order> all, Outer.Inner in, com.acme.model.Outer.Inner out) {
        all.add(null);
        in.go();
        out.go();
        inner.go();
        far.go();
        names.clear();
        entry.getValue().cancel();
        deep[0].dive();
        java.util.Map<String, Order> byId = new java.util.HashMap<>();
        byId.get("a").cancel();
        Outer.Inner local = (Outer.Inner) in;
        local.go();
        Object o = new com.acme.model.Outer.Inner(1);
        if (o instanceof Outer.Inner p && o instanceof java.io.File f) {
            p.go();
            f.delete();
        }
    }

    void inferred(long id) throws java.io.IOException {
        var order = repo.find(id);
        order.cancel();
        var client = this.service.client();
        client.send();
        var made = Foo.of(id);
        made.build();
        var own = make();
        own.go();
        var list = new java.util.ArrayList<Order>();
        list.add(order);
        var grid = new Order[2][3];
        grid[0][1].cancel();
        grid[0].clone();
        var copy = order;
        copy.ship(1, 2);
        var field = service;
        field.client();
        var unknown = id > 0 ? order : null;
        unknown.cancel();
        try (var ch = Channel.open(); var out = ch.sink()) {
            ch.send(null, false);
            out.flush();
        }
        for (var each : order.lines()) {
            each.price();
        }
    }

    void methodRefs(java.util.List<Order> all) {
        all.forEach(Foo::bar);
        all.forEach(repo::save);
        all.forEach(this.service::accept);
        all.forEach(java.util.Objects::requireNonNull);
        all.stream().map(Outer.Inner::wrap).map(com.acme.model.Outer::of).forEach(System.out::println);
        java.util.function.Supplier<Order> s = Order::new;
        Runnable r = Contract.super::close;
        handler = Foo::bar;
        register(Foo::baz, this::close);
    }

    void statics(Order x) {
        require(x != null);
        check(MAX_ORDERS);
        int n = MAX_ORDERS + Limits.MIN_ORDERS + com.acme.util.Limits.STEP;
        Foo.count = n;
        Foo.Registry.INSTANCE.add(x);
        Outer.Inner.DEFAULT.go();
        com.acme.util.Limits.Range.of(1, 2).clamp(n);
    }

    Object extras(Object o, Outer outer) {
        outer.new Inner().go();
        Unit.B.toBytes(1);
        Runnable r = new Runnable() {
            Order own() {
                return null;
            }

            public void run() {
                own();
                own().cancel();
                statics(null);
            }
        };
        return switch (o) {
            case final Order ord -> ord.total();
            case Outer.Inner in when in.ok() -> in.go();
            default -> null;
        };
    }

    @Override
    public void close() throws java.io.IOException {
        throw new java.io.IOException("closed", new com.acme.Failure());
    }

    class Nested extends Outer.Inner implements com.acme.model.Outer.Listener {
        Nested() {
            super(1);
        }

        void run() {
            repo.find(1).cancel();
            close();
            go();
            this.go();
            super.go();
            Contract.this.service.client().send();
        }
    }
}
