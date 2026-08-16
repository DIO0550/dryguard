//! 構文だけを見る層（計画の Stage 1: 候補抽出）。
//!
//! チャンク化・AST 正規化・類似度・import 収集を担う。**LSP を知らない**ので、
//! サーバが入っていない環境でもここまでは動く（`rules/architecture.md`「依存方向のルール」）。
//!
//! 意味情報を見る層は `semantics`、判定は `classification` が持つ。
//! **このツールの主張（構文が似ていることと意味が同じことは別物）が、
//! そのままモジュールの分かれ目になっている。**
//!
//! Phase 0 では、指定位置を含む関数をソースから切り出すところまでがある。

pub mod chunk;
pub mod line_range;
pub mod token;
