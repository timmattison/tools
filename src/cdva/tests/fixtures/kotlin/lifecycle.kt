package com.example.ledger

import org.junit.jupiter.api.BeforeEach

class Ledger {
    var total: Int = 0

    fun record(amount: Int) {
        total += amount
    }
}

class LedgerSpec {
    private val ledger = Ledger()

    @BeforeEach
    fun reset() {
        ledger.total = 0
    }
}
