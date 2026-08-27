//! LSP サーバとの会話。子プロセスの起動と JSON-RPC に閉じる。
//!
//! 複数のステージを組み合わせる手順はここには置かない
//! (rules/architecture.md「依存方向のルール」)。

pub mod framing;
pub mod message;
