package com.acme.app

import com.acme.shop.Order
import com.acme.shop.Order as ShopOrder
import com.acme.shop.Registry.register
import com.acme.shop.Order.Companion.create
import com.acme.shop.describe
import com.acme.util.*
import com.acme.shop.topLevel as make
import kotlin.math.max
import java.util.concurrent.ConcurrentHashMap

fun main() {
    val a = Order(1, "a")
    val b = ShopOrder(2, "b")
    b.describe()
    register { }
    create(3).total
    make(4, "c")
    max(1, 2)
    val map = ConcurrentHashMap<String, ShopOrder>()
    map.put("x", a)
    val c: ShopOrder = b
    c.name
    com.acme.shop.Order.create(5)
    val d = com.acme.shop.Order(6, "f")
    d.total
    java.util.UUID.randomUUID().toString()
}
