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
//! 現在は Phase 1 の途中。CLI の受け口（`compare` と `scan`）、`codebase` の対象ファイル
//! 収集、`syntax` のチャンク化・構造類似度・import 収集、`classification` の
//! ハードコードした閾値による 3 ラベルの判定、`report` の理由付き text 出力までがある。
//! `semantics` と `lsp` はまだ無い。
//!
//! `syntax` はチャンク化・AST 正規化・import 収集のすべてを tree-sitter の構文木から採る。
//! 構造類似度は、正規化トークン列に現れる並びの重なりで測る。

pub mod classification;
pub mod cli;
pub mod codebase;
pub mod line_number;
pub mod location;
pub mod pipeline;
pub mod report;
pub mod similarity;
pub mod syntax;
pub mod threshold;

#[cfg(test)]
mod test_support;
