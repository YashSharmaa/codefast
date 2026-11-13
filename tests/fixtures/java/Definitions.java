/* Definitions: every kind of declaration the Java extractor records. */
@Generated
package com.acme.shop;

import java.util.List;
import java.util.*;
import com.acme.util.Money;
import static com.acme.util.Checks.require;
import static org.junit.Assert.*;

/** A catalogue of items. */
@Entity(name = "catalog")
public abstract class Catalog<T extends Item> extends Store<T> implements Searchable, java.io.Closeable {
    public static final int MAX_ITEMS = 100;
    private static final Logger LOG = Logger.get(Catalog.class);
    protected Map<String, List<T>> index = new HashMap<>();
    int[] counts, shelves[];
    @Nullable private volatile java.util.Set<String> tags;
    final Runnable onClose = this::flush;

    public Catalog(String name, Money... prices) {
        super(name);
        this.tags = null;
    }

    private Catalog() {
        this("anonymous");
    }

    public abstract T lookup(String id) throws NotFoundException, java.io.IOException;

    protected native long checksum(byte[] data);

    @Override
    public final <R extends Comparable<R>> List<R> sorted(Function<? super T, ? extends R> key, int limit) {
        return null;
    }

    static String describe(Object... parts) {
        return String.valueOf(parts.length);
    }

    void flush() {}

    static {
        register(MAX_ITEMS);
    }

    {
        flush();
    }

    /** Nested types. */
    public static class Entry<K> {
        private final K key;

        Entry(K key) {
            this.key = key;
        }

        K key() {
            return key;
        }
    }

    protected interface Visitor<R> {
        int DEPTH = 3;

        R visit(Item item);

        default R visitAll(List<Item> items) {
            return null;
        }

        static Visitor<Void> noop() {
            return null;
        }

        private void helper() {}
    }

    enum Status implements Labeled {
        ACTIVE("a"),
        @Deprecated
        RETIRED("r") {
            @Override
            String label() {
                return "gone";
            }
        },
        UNKNOWN;

        private final String code;

        Status(String code) {
            this.code = code;
        }

        Status() {
            this("?");
        }

        String label() {
            return code;
        }
    }

    public record Price(long cents, @Nullable String currency, List<String>... notes) implements Comparable<Price> {
        static final Price FREE = new Price(0, null);

        public Price {
            if (cents < 0) throw new IllegalArgumentException(currency);
        }

        Price(long cents) {
            this(cents, "EUR");
        }

        public int compareTo(Price other) {
            return Long.compare(cents, other.cents);
        }
    }

    public @interface Audited {
        String value() default "";

        int[] levels() default {};

        Class<? extends Item> type();

        int VERSION = 2;
    }

    Object listeners() {
        class Counter implements Runnable {
            int n;

            public void run() {
                n++;
            }
        }
        return new Visitor<String>() {
            String last;

            @Override
            public String visit(Item item) {
                last = item.name();
                return last;
            }
        };
    }
}

sealed interface Shape permits Circle, Square {}

class CatalogTests {
    @Test
    public void checksLookup() {}

    @org.junit.jupiter.api.ParameterizedTest
    void withParams(int n) {}

    public void testFlush() {}

    void helper() {}
}

class Plain {
    void testNothing() {}
}
