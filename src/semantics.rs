//! 意味情報を集める層（計画の Stage 2: 意味情報収集）。
//!
//! LSP へ問い合わせて返った応答を、判定が使える形へ正規化する。**判定はしない**
//! （`rules/architecture.md`「3 ステージのパイプライン」）。ラベルを決めるのは
//! `classification` にしか無い。
//!
//! | モジュール | 持つもの |
//! |---|---|
//! | `resolved_type` | シグネチャに書かれた型名を、それが指す型の綴りへ解決する |
//! | `type_signature` | hover に尋ね、返った綴りを単一化の可否を比べられる形へ直す |
//! | `caller_domain` | references に尋ね、参照元がどのドメインに属するかを数える |
//!
//! **サーバを起こす・握手する・ドキュメントを開かせるのはここではない。**
//! 借りた `lsp::Session` に尋ねるだけで、その手順は `pipeline` が持つ
//! （`rules/architecture.md`「`lsp` はオーケストレーションを持たない」）。
//!
//! 呼び出し先（callHierarchy）の収集はこの後の Phase で足す。

pub mod caller_domain;
pub mod resolved_type;
pub mod type_signature;
