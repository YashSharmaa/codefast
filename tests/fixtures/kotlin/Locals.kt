package com.acme.locals

class Totals(private val orders: List<Order>, start: Int) {
    private var sum = start
    val doubled = start * 2

    init {
        sum += start
    }

    fun compute(limit: Int, vararg extra: Order): Int {
        var total = 0
        val first: Order = orders.first()
        first.check()
        for (order in orders) {
            total += order.amount
            order.check()
        }
        for ((index, order) in orders.withIndex()) {
            index.toString()
            order.check()
        }
        val (id, amount) = first
        id.inc()
        orders.forEach { it.check() }
        orders.map { o -> o.amount }
        orders.forEachIndexed { i, o: Order -> o.check() }
        try {
            first.check()
        } catch (e: IllegalStateException) {
            e.printStackTrace()
        }
        when (val s = first.status) {
            Status.OPEN -> s.close()
            else -> limit
        }
        val fn = { x: Int -> x + limit }
        fn(total)
        sum = total
        return total + amount + extra.size
    }
}

class Order(val amount: Int, val status: Status) {
    fun check(): Boolean = amount > 0
    operator fun component1() = 1
    operator fun component2() = amount
}

enum class Status {
    OPEN, CLOSED;

    fun close() = CLOSED
}
