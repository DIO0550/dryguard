import Foundation

extension Date {
    func testTimestamp() -> Bool {
        let timestamp = timeIntervalSinceReferenceDate
        let double = timestamp * 2
        return double.isFinite
    }

    func timestampIsFinite() -> Bool {
        let timestamp = timeIntervalSince1970
        let double = timestamp * 2
        return double.isFinite
    }
}
