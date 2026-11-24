import XCTest

struct Calculator {
    func add(_ a: Int, _ b: Int) -> Int {
        return a + b
    }
}

final class CalculatorChecks: XCTestCase {
    func testAddsTwoNumbers() {
        XCTAssertEqual(Calculator().add(1, 2), 3)
    }
}
