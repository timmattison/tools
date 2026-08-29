import XCTest

struct Greeter {
    func greet() -> String {
        return "こんにちは"
    }
}

final class GreeterChecks: XCTestCase {
    func testGreetsInJapanese() {
        // 挨拶を確かめる
        XCTAssertEqual(Greeter().greet(), "こんにちは")
    }
}
