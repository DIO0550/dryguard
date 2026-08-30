//! 意味情報を集める層（計画の Stage 2: 意味情報収集）。
//!
//! LSP へ問い合わせて返った応答を、判定が使える形へ正規化する。**判定はしない**
//! （`rules/architecture.md`「3 ステージのパイプライン」）。ラベルを決めるのは
//! `classification` にしか無い。
//!
//! | モジュール | 持つもの |
//! |---|---|
//! | `type_signature` | hover が返した綴りを、単一化の可否を比べられる形へ直す |
//! | `caller_domain` | 参照元がどのドメインに属するかと、その重なり |
//!
//! 呼び出し先（callHierarchy）の収集はこの後の Phase で足す。

pub mod caller_domain;
pub mod type_signature;
