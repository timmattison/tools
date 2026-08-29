package com.example.greet;

import static org.junit.jupiter.api.Assertions.assertEquals;

import org.junit.jupiter.api.Test;

class Multibyte {
    String greet() {
        return "こんにちは";
    }

    @Test
    void greetsInJapanese() {
        // 挨拶を確かめる
        assertEquals("こんにちは", greet());
    }
}
