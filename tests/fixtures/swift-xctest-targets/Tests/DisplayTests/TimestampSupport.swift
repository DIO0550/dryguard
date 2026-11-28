import Foundation
import XCTest

extension Date {
    func testFixtureTimestamp() -> Bool {
        let timestamp = timeIntervalSinceReferenceDate
        let double = timestamp * 2
        return double.isFinite
    }
}

final class DisplayTests: XCTestCase {
    func testFiniteDate() {
        XCTAssertTrue(Date().testFixtureTimestamp())
    }
}
