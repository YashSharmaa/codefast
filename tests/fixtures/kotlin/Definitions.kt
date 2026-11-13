@file:JvmName("Shop")
package com.acme.shop

typealias Handler = (Order) -> Unit
typealias OrderList = List<Order>

const val MAX_ITEMS = 10
val registry: Registry = Registry()
private var counter = 0

fun topLevel(a: Int, b: String = "x", vararg rest: Int): Order = Order(a, b)

internal fun helper() = Order(1, "h")

fun Order.describe(): String = name

val Order.isBig: Boolean
    get() = id > MAX_ITEMS

expect fun platform(): String

suspend fun fetch(id: Int): Order? = null

data class Order(val id: Int, var name: String, private val note: String = "") : Base(id), Comparable<Order> {
    val total: Int
        get() = id * 2
    var label: String = ""
        private set(value) {
            field = value.trim()
        }
    lateinit var owner: Customer
    private val cache = mutableMapOf<Int, Order>()

    constructor(id: Int) : this(id, "n")

    init {
        require(id >= 0)
    }

    override fun compareTo(other: Order): Int = id - other.id

    fun withLocal(): Int {
        fun twice(x: Int) = x * 2
        return twice(id)
    }

    companion object {
        const val PREFIX = "o-"
        fun create(id: Int): Order = Order(id, PREFIX)
    }

    inner class Line(val qty: Int)

    class Builder {
        fun build(): Order = Order(0)
    }
}

class Customer(val name: String) {
    companion object Factory : Maker<Customer> {
        override fun make(): Customer = Customer("anon")
    }
}

interface Maker<T> {
    fun make(): T
    val label: String
    fun describe(): String = "maker"
}

fun interface Callback {
    fun call(order: Order)
}

sealed class Shape {
    abstract fun area(): Double
    object Empty : Shape() {
        override fun area() = 0.0
    }
}

enum class Color(val rgb: Int) {
    RED(0xff0000),
    GREEN(0x00ff00) {
        override fun warm() = false
    };

    open fun warm() = this == RED
}

annotation class Audited(val by: String, val level: Int = 1)

abstract class Base(val baseId: Int) {
    protected abstract fun check(): Boolean
    open fun run() {}
}

@JvmInline
value class OrderId(val value: Int)

object Registry {
    private val handlers = mutableListOf<Handler>()
    fun register(h: Handler) {
        handlers.add(h)
    }
}

class OrderTest {
    @Test
    fun createsOrder() {
        assertEquals(1, Order.create(1).id)
    }

    @org.junit.jupiter.api.Test
    fun qualifiedAnnotation() {}

    fun notATest() {}
}
