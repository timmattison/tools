package com.example.calc;

import static org.junit.jupiter.api.Assertions.assertTrue;

import org.junit.jupiter.api.RepeatedTest;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.ValueSource;

class Parameterized {
    int doubled(int value) {
        return value * 2;
    }

    @ParameterizedTest
    @ValueSource(ints = {1, 2})
    void doublesEveryInput(int value) {
        assertTrue(doubled(value) > value);
    }

    @RepeatedTest(3)
    void doublesTheSameWayEveryTime() {
        assertTrue(doubled(2) == 4);
    }
}
