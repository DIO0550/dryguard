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
