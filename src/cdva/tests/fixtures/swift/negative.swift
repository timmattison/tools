protocol Computing {
    func add(_ a: Int, _ b: Int) -> Int
}

final class Calculator: Computing {
    func add(_ a: Int, _ b: Int) -> Int {
        return a + b
    }
}

func check() {
    precondition(Calculator().add(1, 2) == 3)
}
