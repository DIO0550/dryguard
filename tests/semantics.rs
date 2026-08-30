//! 候補ペアのチャンクから、LSP へ問い合わせて型シグネチャと参照元を比べるところまで。
//!
//! ステージをつないだ結果はここで見る（rules/testing.md「ステージをまたぐテストと
//! 単体のテストを分ける」）。名前の位置は `syntax::chunk`、応答の読み取りは
//! `lsp::hover` / `lsp::references`、正規化と比較は `semantics::type_signature` /
//! `semantics::caller_domain` のモジュール内テストにある。
//!
//! **どのテストも実サーバを要するので `#[ignore]` を付ける。** サーバの入っていない
//! 開発機で黙って通さないため（rules/testing.md「LSP を要するテストは、飛ばしたことが
//! 分かる形にする」）。CI はサーバを入れて `--ignored` で走らせる。

use std::path::{Path, PathBuf};

use dryguard::codebase::source_of;
use dryguard::location::Location;
use dryguard::lsp::{
    Client, HoverOutcome, ReferencesOutcome, ServerCommand, Session, SourceDocument, WorkspaceRoot,
};
use dryguard::pipeline::chunk_pair_of;
use dryguard::semantics::caller_domain::CallerDomains;
use dryguard::semantics::type_signature::TypeSignature;
use dryguard::syntax::chunk::Chunk;

/// 参照元を持つフィクスチャの木のプロジェクト設定。
///
/// **サーバに見せる根はここを含む位置にする。** tsconfig.json より下を根にすると、
/// サーバは開いたファイルとその import 先だけでプロジェクトを組み立てる。呼び出し元は
/// import を辿る向きの逆にあるので、**一部しか返らない**。
const THE_REFERENCES_PROJECT_FILE: &str = "references/tsconfig.json";

/// `tests/fixtures/` 配下の位置。
///
/// カレントディレクトリではなくマニフェストの位置から組み立てる
/// （テストの実行位置に依存させない）。
///
/// `expect` ではなく `panic!` で落とすのは、`clippy.toml` の `allow-expect-in-tests` が
/// `#[test]` の関数と `#[cfg(test)]` のモジュールしか見ないため。統合テストの
/// ヘルパー関数はそのどちらでもなく、`expect` は本番コードと同じく落とされる。
fn fixture(relative_path: &str, line: usize) -> Location {
    let text = format!(
        "{}/tests/fixtures/{relative_path}:{line}",
        env!("CARGO_MANIFEST_DIR")
    );

    let Ok(location) = text.parse() else {
        panic!("テストが組み立てる位置は解釈できる: {text}");
    };
    location
}

/// そのファイルを、サーバに開かせる形にする。
fn document(path: &Path) -> SourceDocument {
    let Ok(text) = source_of(path) else {
        panic!("テストが指すファイルは読める: {}", path.display());
    };
    let Ok(document) = SourceDocument::new(path, text) else {
        panic!("テストが指すファイルは開かせられる: {}", path.display());
    };
    document
}

/// 2 つのチャンクを開かせられる、握手を終えたサーバ。
fn session_over(paths: &[PathBuf]) -> Session {
    let Ok(root) = WorkspaceRoot::enclosing(paths) else {
        panic!("テストが渡すパスからは根を決められる");
    };
    let Ok(client) = Client::start(&ServerCommand::typescript()) else {
        panic!("typescript-language-server を起動できる");
    };
    let Ok(session) = client.handshake(&root) else {
        panic!("サーバと握手できる");
    };
    session
}

/// そのチャンクの型シグネチャを、サーバに尋ねて正規化したもの。
fn type_signature_of(session: &mut Session, chunk: &Chunk) -> TypeSignature {
    let document = document(chunk.path());
    if session.open_document(&document).is_err() {
        panic!("ファイルを開かせられる: {}", chunk.path().display());
    }

    let Some(position) = chunk.name_position() else {
        panic!(
            "テストが指すチャンクは名前を持つ: {}",
            chunk.path().display()
        );
    };
    let Ok(HoverOutcome::Answered(signature_text)) = session.hover(&document, position) else {
        panic!("名前の位置には hover が答える: {}", chunk.path().display());
    };
    let Some(signature) = TypeSignature::from_signature_text(&signature_text) else {
        panic!("サーバが返した綴りは読み取れる: {signature_text}");
    };
    signature
}

/// 2 箇所のチャンクの型シグネチャが単一化できるか、実サーバに尋ねて確かめる。
fn unifiable(location_a: &Location, location_b: &Location) -> bool {
    let Ok((chunk_a, chunk_b)) = chunk_pair_of(location_a, location_b) else {
        panic!("テストが渡す位置はどちらも関数の中を指している");
    };

    let mut session = session_over(&[
        location_a.path().to_path_buf(),
        location_b.path().to_path_buf(),
    ]);
    let signature_a = type_signature_of(&mut session, &chunk_a);
    let signature_b = type_signature_of(&mut session, &chunk_b);
    let unifiable = signature_a.is_unifiable_with(&signature_b);

    if session.shutdown().is_err() {
        panic!("サーバを終わらせられる");
    }
    unifiable
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_two_date_formatting_functions_have_unifiable_type_signatures() {
    // 計画の出力イメージで EXTRACT-CANDIDATE 側に置かれているペア
    // （`docs/dryguard-plan.md`「出力イメージ」）。どちらも `(Date) => string`
    let formats_a_date = fixture("utils/formatDate.ts", 3);
    let helps_with_dates = fixture("report/dateHelper.ts", 3);

    assert!(unifiable(&formats_a_date, &helps_with_dates));
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_two_functions_taking_types_from_separate_domains_are_not_unifiable() {
    // 対照は上のテスト。構造は同じだが、受け取る型が別ドメインのもので
    // `(Invoice) => number` と `(Stock) => number` になる
    let discounts_an_invoice = fixture("billing/discount.ts", 5);
    let reorders_stock = fixture("inventory/reorder.ts", 5);

    assert!(!unifiable(&discounts_an_invoice, &reorders_stock));
}

/// そのチャンクの呼び出し元が属するドメイン。サーバに尋ねて数える。
fn caller_domains_of(session: &mut Session, chunk: &Chunk) -> CallerDomains {
    let document = document(chunk.path());
    if session.open_document(&document).is_err() {
        panic!("ファイルを開かせられる: {}", chunk.path().display());
    }

    let Some(position) = chunk.name_position() else {
        panic!(
            "テストが指すチャンクは名前を持つ: {}",
            chunk.path().display()
        );
    };
    let Ok(ReferencesOutcome::Answered(reference_paths)) = session.references(&document, position)
    else {
        panic!("名前の位置には参照元が返る: {}", chunk.path().display());
    };
    let Some(caller_domains) = CallerDomains::from_reference_paths(&reference_paths) else {
        panic!("返った参照元は 1 件以上ある: {reference_paths:?}");
    };
    caller_domains
}

/// 2 箇所のチャンクの呼び出し元ドメインがどれだけ重なるか、実サーバに尋ねて測る。
fn caller_domain_overlap(location_a: &Location, location_b: &Location) -> f64 {
    let Ok((chunk_a, chunk_b)) = chunk_pair_of(location_a, location_b) else {
        panic!("テストが渡す位置はどちらも関数の中を指している");
    };

    let mut session = session_over(&[
        location_a.path().to_path_buf(),
        location_b.path().to_path_buf(),
        fixture(THE_REFERENCES_PROJECT_FILE, 1).path().to_path_buf(),
    ]);
    let domains_a = caller_domains_of(&mut session, &chunk_a);
    let domains_b = caller_domains_of(&mut session, &chunk_b);
    let overlap = domains_a.jaccard(&domains_b).value();

    if session.shutdown().is_err() {
        panic!("サーバを終わらせられる");
    }
    overlap
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_two_functions_called_from_separate_domains_share_no_caller_domains() {
    // 計画の出力イメージで DO-NOT-EXTRACT 側に置かれているペア
    // （`docs/dryguard-plan.md`「出力イメージ」）。applyDiscount は billing の
    // 2 ファイルから、reorderAmount は inventory の 1 ファイルから呼ばれている
    let discounts_an_invoice = fixture("references/src/billing/discount.ts", 5);
    let reorders_stock = fixture("references/src/inventory/reorder.ts", 5);

    assert_eq!(
        caller_domain_overlap(&discounts_an_invoice, &reorders_stock),
        0.0
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_two_functions_called_from_the_same_domain_share_their_caller_domains() {
    // 対照は上のテスト。**ディレクトリは utils と report で分かれている**が、
    // どちらも report/monthly.ts から呼ばれている。ここが Phase 0 の
    // ディレクトリ距離との違いで、置き場所ではなく実際に誰が使っているかを見る
    let formats_a_date = fixture("references/src/utils/formatDate.ts", 3);
    let helps_with_dates = fixture("references/src/report/dateHelper.ts", 3);

    assert_eq!(
        caller_domain_overlap(&formats_a_date, &helps_with_dates),
        1.0
    );
}
