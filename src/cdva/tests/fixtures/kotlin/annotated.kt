package com.example.calc

import kotlin.test.Test
import kotlin.test.assertEquals

fun add(a: Int, b: Int): Int = a + b

class AdditionSpec {
    @Test
    fun addsTwoNumbers() {
        assertEquals(3, add(1, 2))
    }
}
