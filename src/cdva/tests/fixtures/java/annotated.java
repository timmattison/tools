package com.example.calc;

import static org.junit.jupiter.api.Assertions.assertEquals;

import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

/** A calculator that carries a base the tests reset. */
class Annotated {
    private int base;

    int add(int a, int b) {
        return a + b + base;
    }

    @BeforeEach
    void reset() {
        base = 0;
    }

    @Test
    void addsTwoNumbers() {
        assertEquals(3, add(1, 2));
    }
}
