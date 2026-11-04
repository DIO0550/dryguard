/// `tests/swift_lsp.rs` が型シグネチャを比べる関数。本体の違いは型シグネチャに効かない。
func doubled(value: Int) -> Int {
    value * 2
}

func tripled(value: Int) -> Int {
    value * 3
}

func scaled(by value: Int) -> Int {
    value * 2
}

func halved(value: Double) -> Int {
    Int(value / 2)
}

func unwrapped(value: Int?) -> Int {
    value ?? 0
}

func defaulted(value: Optional<Int>) -> Int {
    value ?? 1
}

/// 制約付きの extension のメンバー。hover は `where` を綴らない（sourcekit-lsp 6.4.2）。
extension Array where Element == Int {
    func summed(by scale: Int) -> Int {
        reduce(0, +) * scale
    }
}

extension Array where Element == Int {
    func totalled(by factor: Int) -> Int {
        reduce(0, +) * factor
    }
}

extension Array where Element == Double {
    func weighted(by scale: Int) -> Int {
        Int(reduce(0, +)) * scale
    }
}

extension Array where Element == [Int] {
    func flattened(by scale: Int) -> Int {
        joined().reduce(0, +) * scale
    }
}

extension Array<Int> {
    func counted(by scale: Int) -> Int {
        count * scale
    }
}

extension [Int] {
    func sugared(by scale: Int) -> Int {
        count * scale
    }
}
