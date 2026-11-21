import XCTest
@testable import Shop

extension CheckoutTests {
    func testTotal() {
        XCTAssertEqual(total([1, 2]), 3)
    }
}
