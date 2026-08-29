import Testing

func doubled(_ value: Int) -> Int {
    return value * 2
}

@Test
func doublesTheInput() {
    precondition(doubled(2) == 4)
}
