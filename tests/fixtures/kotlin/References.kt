package com.acme.shop

import com.acme.base.Repo
import com.acme.io.Channel as Chan

class Service(private val repo: Repo, val chan: Chan) : Runnable, Callback {
    private val audit = AuditLog()
    var onDone: (Order) -> Unit = ::log

    override fun run() {}

    override fun call(order: Order) {
        val found = repo.find(order.id)
        found.save()
        repo.find(1).save()
        this.repo.flush()
        audit.record(order)
        order?.describe()
        super.toString()
        Order.create(2)
        Customer.make()
        Customer.Factory.make()
        Registry.register(::log)
        Registry.register(this::handle)
        Registry.register { o -> o.total }
        chan.send(order.name)
        val first = Order(1, "a")
        first.name = "b"
        first.label
        val line = first.Line(3)
        val built = Order.Builder().build()
        built.compareTo(first)
        val cast = repo as AuditLog
        cast.record(first)
        if (repo is AuditLog) println("audit")
        process(order) { it.total }
        handle(order)
        topLevel(1, b = "q")
        helper().describe()
        first pairWith built
        1 to 2
        onDone = ::log
        onDone(first)
        val maker: Maker<Order> = OrderMaker()
        maker.make().total
        log(first)
    }

    fun handle(o: Order) {
        o.apply { describe() }
        o.also { it.compareTo(o) }
        with(o) { name }
        o.let { x -> x.total }
        Order(5, "z").apply { label = "done" }.describe()
    }

    private fun process(o: Order, f: (Order) -> Int): Int = f(o)

    infix fun Order.pairWith(other: Order) = this.id + other.id
}

fun log(o: Order) {
    println(o.name)
    o.describe()
    registry.register { }
}

class OrderMaker : Maker<Order> {
    override val label = "orders"
    override fun make(): Order = Order.create(0)
}

class AuditLog {
    fun record(o: Order) {}
}

class Repository<T : Order>(private val items: MutableList<T>) {
    fun firstTotal(): Int = items.first().total
    fun <R : Comparable<R>> sortedBy(key: (T) -> R): List<T> = items.sortedBy(key)
    fun touch(t: T) = t.describe()
}
