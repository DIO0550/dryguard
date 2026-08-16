//! 構造の似たコードが偶発的な重複かどうかを、型・依存・参照の意味情報から理由付きで判定する。
//!
//! 3 ステージのパイプラインで構成する
//! (`docs/dryguard-plan.md`「アーキテクチャ」/ `rules/architecture.md`)。
//!
//! | ステージ | 責務 |
//! |---|---|
//! | Stage 1 | 候補抽出（チャンク化・正規化・類似度・import 収集） |
//! | Stage 2 | 意味情報収集（LSP への問い合わせと結果の正規化） |
//! | Stage 3 | 分類（シグナルの統合と判定・理由の組み立て） |
//!
//! 現在は Phase 0 の途中で、CLI の受け口と Stage 1 のチャンク化までがある。

pub mod cli;
pub mod location;
pub mod pipeline;
pub mod stage1;
pub mod threshold;

#[cfg(test)]
mod test_support;
