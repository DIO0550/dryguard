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
//! 現在は Phase 0 の途中。CLI の受け口、`syntax` のチャンク化・構造類似度・import 収集、
//! `classification` のハードコードした閾値による 3 ラベルの判定までがある。
//! `semantics` と `lsp` はまだ無い。

pub mod classification;
pub mod cli;
pub mod line_number;
pub mod location;
pub mod pipeline;
pub mod similarity;
pub mod syntax;
pub mod threshold;

#[cfg(test)]
mod test_support;
