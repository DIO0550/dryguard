import XCTest

extension Date {
    func testTimestamp() {
        let timestamp = 24 * 60
        let seconds = timestamp * 60
        XCTAssertEqual(seconds, 86400)
    }
}
