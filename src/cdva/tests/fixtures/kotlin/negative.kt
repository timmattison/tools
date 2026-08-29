package com.example.ledger

fun helper(values: List<Int>): Int {
    return values.sum()
}

class Register {
    fun record(amount: Int): Int = amount
}
