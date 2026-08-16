//! Stage 1: 候補抽出。
//!
//! チャンク化・AST 正規化・類似度・import 収集を担う。**LSP を知らない**ので、
//! サーバが入っていない環境でもここまでは動く（`rules/architecture.md`「依存方向のルール」）。
//!
//! Phase 0 では、指定位置を含む関数をソースから切り出すところまでがある。

pub mod chunk;
pub mod line_range;
