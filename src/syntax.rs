//! 構文だけを見る層（計画の Stage 1: 候補抽出）。
//!
//! チャンク化・AST 正規化・類似度・import 収集を担う。**LSP を知らない**ので、
//! サーバが入っていない環境でもここまでは動く（`rules/architecture.md`「依存方向のルール」）。
//!
//! 意味情報を見る層は `semantics`、判定は `classification` が持つ。
//! **このツールの主張（構文が似ていることと意味が同じことは別物）が、
//! そのままモジュールの分かれ目になっている。**
//!
//! チャンクの範囲と import の指定子は tree-sitter の構文木から採る（`tree`）。
//! **どのノードがチャンク / import かという語彙は `chunk` と `import` が持ち**、
//! 木を歩く手順だけを `tree` に置く。
//!
//! 構造類似度は Phase 0 のまま、文字を前から見て切った正規化トークン集合の
//! Jaccard 係数で出している（`token`）。

pub mod chunk;
pub mod import;
pub mod line_range;
pub mod module_distance;
pub(crate) mod source_character;
pub mod token;
pub mod tree;
