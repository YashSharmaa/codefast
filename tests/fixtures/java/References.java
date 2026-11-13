package com.acme.shop;

import com.acme.io.Channel;
import com.acme.io.URL;
import java.io.IOException;
import static com.acme.util.Checks.require;

/** References: call forms, member access, type uses and locals. */
@Service("orders")
public class Orders extends Base<Order> implements Handler<Order>, Runnable {
    private final Repo repo;
    private Channel[] channels;
    private int count;
    static final Orders EMPTY = new Orders(null);
    Consumer<Order> sink;

    Orders(Repo repo) {
        super(repo, 1);
        this.repo = repo;
        this.sink = this::accept;
        sink = Orders::log;
    }

    void calls(Order order, List<Order> all) throws IOException {
        validate(order);
        this.validate(order);
        super.close();
        order.ship(1, 2);
        Repo local = this.repo;
        local.save(order);
        repo.save(order);
        this.repo.find(1).cancel();
        Orders.log(order);
        Money.of(1).plus(Money.ZERO).cents();
        com.acme.util.Checks.require(order != null);
        require(all.isEmpty());
        EMPTY.run();
        LIMITS.check(count);
        URL.parse("x").open();
        System.out.println(String.valueOf(count));
        channels[0].send(order, /* urgent */ true);
        ((Order) all.get(0)).cancel();
        "id".concat(order.id);
        new Order(order).ship(3, 4);
        Collections.<Order>emptyList().size();
        register(Setting.Property.NodeScope, Double.NaN);
    }

    <T extends Shippable> void generics(T item, Map<String, ? super Order> byId) {
        item.ship();
        T copy = item;
        List<T> queue = new ArrayList<>();
        Map.Entry<String, Order> first = null;
        java.util.Optional<Order> maybe = java.util.Optional.empty();
        Class<?> k = Order.class;
        Object raw = (Shippable) item;
        FST.Arc<T> arc = FST.root(new Lookup.T(item));
    }

    Object creation(Order order) {
        Order copy = new Order(order, 2);
        Order.Line line = new Order.Line();
        Order.Line inner = order.new Line();
        int[] sizes = new int[3];
        Order[] many = new Order[] { copy };
        Handler<Order> anon = new Handler<Order>() {
            @Override
            public void accept(Order o) {
                count++;
                validate(o);
                this.accept(o);
            }
        };
        return new Object();
    }

    void methodRefs(List<Order> all) {
        all.forEach(this::accept);
        all.forEach(Orders::log);
        all.forEach(repo::save);
        all.stream().map(Order::new).map(Order.Line::of).toArray(Order[]::new);
        Supplier<Object> s = super::toString;
    }

    void fields(Order order, Orders other) {
        int n = count + this.count + other.count + order.total.cents;
        count = n;
        this.count += 1;
        other.count--;
        order.total.cents = MAX;
        channels[n] = null;
        int len = channels.length + Limits.DEFAULT.max;
    }

    void locals(Object value, List<Order> all) {
        for (int i = 0; i < all.size(); i++) {
            Order each = all.get(i);
            each.ship(i, 0);
        }
        for (Order o : all) {
            o.cancel();
        }
        try (Channel ch = Channel.open(); var out = new Sink(ch)) {
            ch.send(null, false);
            out.flush();
        } catch (IOException | IllegalStateException e) {
            e.printStackTrace();
        } catch (Failure f) {
            f.report();
        } finally {
            reset();
        }
        if (value instanceof Order o && o.isOpen()) {
            o.cancel();
        }
        all.forEach(x -> x.cancel());
        all.forEach((Order x) -> x.ship(1, 1));
        BiFunction<Order, Integer, Order> f = (a, b) -> a;
        var typed = new Order(null, 0);
        typed.cancel();
        var unknown = all.get(0);
        unknown.cancel();
        Runnable r = () -> count++;
        Runnable later = () -> {
            Order tmp = all.get(1);
            tmp.cancel();
        };
    }

    @Override
    public void run() {}
}
