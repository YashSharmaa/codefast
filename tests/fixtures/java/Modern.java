package com.acme.shop;

import static com.acme.shop.Modern.Level.HIGH;

/** Newer syntax and the corner cases of name resolution. */
public class Modern {
    sealed interface Shape permits Circle, Square {}

    record Circle(double radius) implements Shape {}

    record Square(double side) implements Shape {}

    enum Level {
        LOW,
        HIGH;

        Level next() {
            return this == LOW ? HIGH : LOW;
        }
    }

    interface Named {
        default String name() {
            return "n";
        }
    }

    private Level level;
    private final Map<String, Shape> shapes = new HashMap<>();
    static final Comparator<Shape> BY_AREA = (a, b) -> Double.compare(area(a), area(b));
    static final Object PROBE = new Named() {
        public String name() {
            return BY_AREA.toString();
        }
    };

    static double area(Shape shape) {
        return switch (shape) {
            case Circle c -> Math.PI * c.radius() * c.radius();
            case Square(double side) -> side * side;
        };
    }

    String describe(Object o) {
        if (o instanceof Circle(double r) && r > 0) {
            return "circle " + r;
        }
        if (!(o instanceof Square sq)) {
            return "other";
        }
        return switch (level) {
            case LOW -> "low " + sq.side();
            case HIGH -> {
                String text = """
                    high""";
                yield text.strip();
            }
        };
    }

    int oldSwitch(Level l, String s) {
        switch (l) {
            case LOW:
                return 1;
            case HIGH:
            default:
                break;
        }
        switch (s) {
            case NAME:
                return 2;
        }
        return HIGH.ordinal() + Level.LOW.next().ordinal() + Limits.Defaults.MAX.value;
    }

    static Predicate<Level> matcher(Level wanted) {
        return switch (wanted) {
            case LOW -> l -> l == HIGH;
            case HIGH -> (Level l) -> l.next() == wanted;
        };
    }

    void qualifiedPatterns(Object o) {
        switch (o) {
            case Modern.Circle(var radius) -> use(radius);
            case Shapes.Box(Point corner, int size) -> corner.move(size);
            default -> {}
        }
        if (o instanceof Shapes.Box(Point p, int n)) {
            p.move(n);
        }
    }

    class Inner extends Base implements Named {
        int level;

        public String name() {
            level = 1;
            Modern.this.level = HIGH;
            shapes.clear();
            describe(null);
            Modern.super.toString();
            return Named.super.name();
        }
    }

    <T extends Shape & Comparable<T>> T max(List<? extends T> items, T fallback) {
        T best = fallback;
        outer:
        for (T item : items) {
            if (item.compareTo(best) > 0) {
                best = item;
                continue outer;
            }
        }
        best.hashCode();
        return best;
    }

    class Handler<Request, Response extends Result> {
        Response handle(Request request, Modern.Request other) {
            return other.reply(request);
        }

        static Response create(Request request) {
            return request.reply(null);
        }
    }

    @Audited(value = "x", levels = { 1, 2 }, type = Circle.class)
    @Catalog.Marker
    void annotated(@NonNull String s, Catalog.@NonNull Entry<String> e) {
        for (Shape shape : shapes.values()) {
            shape.toString();
        }
        for (Level level : level.all()) {
            level.next();
        }
    }
}
