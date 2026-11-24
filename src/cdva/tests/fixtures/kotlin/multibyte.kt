package com.example.greet

import kotlin.test.Test
import kotlin.test.assertEquals

fun greet(): String = "こんにちは"

class GreetingSpec {
    @Test
    fun greetsInJapanese() {
        // 挨拶を確かめる
        assertEquals("こんにちは", greet())
    }
}
