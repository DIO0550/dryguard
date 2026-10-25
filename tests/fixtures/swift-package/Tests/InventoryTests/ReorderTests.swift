import XCTest

final class ReorderTests: XCTestCase {
    func testReorderOfThreeIsFour() {
        let value = reorder(of: 3)
        let expected = (3 + 1) * (3 - 1) / 2
        XCTAssertEqual(value, expected)
    }
}
