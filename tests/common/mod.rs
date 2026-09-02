//! 統合テストが共有するヘルパー。
//!
//! 統合テストはファイルごとに別のクレートになるので、`src/test_support.rs`
//! （`#[cfg(test)]`）からは見えない。**2 つ以上のテストファイルが同じものを書き始めたら
//! ここへ移す**（rules/testing.md「テスト用ヘルパーの置き場所」）。

use dryguard::lsp::ServerCommand;

/// 起動できない LSP サーバの指定。
///
/// **モックではなく実物の失敗**を使う（`rules/testing.md`「モックは使わない」）。
/// 実行ファイルが無いので `Client::start` が `ServerNotFound` で落ち、
/// **サーバが入っている環境でも入っていない環境でも同じ経路を通る**。
pub fn missing_server() -> ServerCommand {
    ServerCommand::new("dryguard-no-such-language-server", Vec::new())
}
