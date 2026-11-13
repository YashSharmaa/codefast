package com.acme.broken

fun `name with spaces`() = Half().ok()

val lambdaHolder: () -> Unit = { println("x") }

class Generic<in T, out R> where T : Comparable<T> {
    fun apply(t: T): R? = null
    fun bound(t: T) = t.compareTo(t)
}

fun <T> T.selfExt(): T = this

fun String?.orBlank(): String = this ?: ""

fun deep(a: Int) = a + a + a + a + a + a + a + a + a + a + a + a + a + a + a + a + a + a + a + a + a + a

fun calls() {
    val s = """
        multi ${lambdaHolder()} line
    """
    s.orBlank()
    `name with spaces`()
    val g = Generic<String, Int>()
    g.apply("x")
}

class Half {
    fun ok() = 1
    fun broken( {
        val x = ok(
    }
    fun after() = ok()
}
