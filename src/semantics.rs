//! 意味情報を集める層（計画の Stage 2: 意味情報収集）。
//!
//! LSP へ問い合わせて返った応答を、判定が使える形へ正規化する。**判定はしない**
//! （`rules/architecture.md`「3 ステージのパイプライン」）。ラベルを決めるのは
//! `classification` にしか無い。
//!
//! 今あるのは、hover が返した綴りを単一化の可否を比べられる形へ直すところまで
//! （`type_signature`）。呼び出し先・呼び出し元の収集はこの後の Phase で足す。

pub mod type_signature;
