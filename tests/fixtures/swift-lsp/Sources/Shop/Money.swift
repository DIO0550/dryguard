/// `tests/swift_lsp.rs` が型シグネチャで開く `typealias`。使う側（`Signatures.swift`）とは別のファイルに置く。
///
/// 循環する `typealias` は置かない。Shop モジュールがコンパイルエラーになり、ほかの実サーバのテストの
/// index を巻き込みうる（循環した宣言の使用位置には、sourcekit-lsp 6.4.2 が hover も definition も答えない）。
typealias Money = Int

public typealias Cost = Double

typealias Fee = Money

typealias Couple<T> = (T, T)

struct Box<T> {
    typealias Item = T
}
