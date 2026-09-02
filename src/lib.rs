//! 構造の似たコードが偶発的な重複かどうかを、型・依存・参照の意味情報から理由付きで判定する。
//!
//! 3 ステージのパイプラインで構成する
//! (`docs/dryguard-plan.md`「アーキテクチャ」/ `rules/architecture.md`)。
//!
//! | モジュール | 責務 | 計画での呼び名 |
//! |---|---|---|
//! | `syntax` | 候補抽出（チャンク化・正規化・類似度・import 収集） | Stage 1 |
//! | `semantics` | 意味情報収集（LSP への問い合わせと結果の正規化） | Stage 2 |
//! | `classification` | 分類（シグナルの統合と判定・理由の組み立て） | Stage 3 |
//!
//! 現在は Phase 2 の途中。CLI の受け口（`compare` と `scan`）、`codebase` の対象ファイル
//! 収集、`syntax` のチャンク化・構造類似度・import 収集、`classification` の
//! ハードコードした閾値による 3 ラベルの判定、`report` の理由付き text 出力までがある。
//!
//! `lsp` はサーバの起動・JSON-RPC の往復・握手・ワークスペースの根の受け渡し・
//! 候補ペアのファイルの開閉・hover / references の問い合わせを持つ。
//! `semantics` はその応答を正規化し、型シグネチャが単一化できるかと、
//! 呼び出し元がどのドメインから来ているかを出す。
//!
//! **`compare` と `scan` のどちらもその 2 つを判定に使う。** `scan` はサーバを走査に
//! つき 1 度だけ起こし、**候補ペアに現れるチャンクへ 1 回ずつ**尋ねる（1 つのチャンクは
//! 複数のペアに現れるので、ペアごとに尋ねると同じ問い合わせを何度も送ることになる）。
//! 候補ペアが 1 組も無ければサーバを起こさない。
//!
//! `syntax` はチャンク化・AST 正規化・import 収集のすべてを tree-sitter の構文木から採る。
//! 構造類似度は、正規化トークン列に現れる並びの重なりで測る。

pub mod classification;
pub mod cli;
pub mod codebase;
pub mod line_number;
pub mod location;
pub mod lsp;
pub mod pipeline;
pub mod report;
pub mod semantics;
pub mod similarity;
pub mod source_position;
pub mod syntax;
pub mod threshold;

#[cfg(test)]
mod test_support;
