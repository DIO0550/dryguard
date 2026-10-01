//! 実際の rust-analyzer で hover / references を確かめる。
//!
//! サーバを要するので通常の `cargo test` では飛ばし、CI で `--ignored` を実行する。

use std::path::PathBuf;
use std::time::Instant;

use dryguard::classification::ConfiguredThresholds;
use dryguard::codebase::SourceLanguage;
use dryguard::codebase::source_of;
use dryguard::domain_declaration::DomainDeclarations;
use dryguard::line_number::LineNumber;
use dryguard::lsp::{
    Client, HoverOutcome, ReferencesOutcome, ServerCommand, SourceDocument, WorkspaceRoot,
};
use dryguard::pipeline::scan_of_language;
use dryguard::report::{Explanation, scan_text_of};
use dryguard::semantics::resolved_type::TracedTypeNames;
use dryguard::semantics::type_signature::{
    TypeSignatureOutcome, UntracedReason, rust_type_signature_outcome_of,
};
use dryguard::source_position::SourcePosition;
use dryguard::syntax::chunk::TestFunctions;

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_rust_analyzer_answers_both_candidates_in_one_session() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rust-lsp/src/lib.rs");
    let root = WorkspaceRoot::enclosing(std::slice::from_ref(&path)).expect("ワークスペースの根");
    let document = SourceDocument::new(&path, source_of(&path).expect("フィクスチャを読める"))
        .expect("ドキュメントを作れる");

    let started = Instant::now();
    let client = Client::start(&ServerCommand::rust()).expect("rust-analyzer を起動できる");
    let mut session = client.handshake(&root).expect("握手できる");
    let handshake = started.elapsed();
    session.open_document(&document).expect("ファイルを開ける");

    for (line, name) in [(1, "first"), (5, "second")] {
        let position = SourcePosition::from_preceding_text(
            LineNumber::new(line).expect("正の行番号"),
            "pub fn ",
        );
        let hover = session
            .hover(&document, position)
            .expect("hover を尋ねられる");
        let HoverOutcome::Answered(signature) = hover else {
            panic!("{name} の型が返る: {hover:?}");
        };
        assert!(signature.as_str().contains(name), "{name}: {signature:?}");

        let references = session
            .references(&document, position)
            .expect("references を尋ねられる");
        let ReferencesOutcome::Answered(paths) = references else {
            panic!("{name} の参照元が返る: {references:?}");
        };
        assert!(paths.contains(&path), "{name} の呼び出し元: {paths:?}");
    }

    let queried = started.elapsed();
    session.shutdown().expect("正常終了できる");
    eprintln!(
        "rust-analyzer: handshake={handshake:?}, hover/references x2={:?}, total={:?}",
        queried - handshake,
        started.elapsed()
    );
}

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_rust_analyzer_indexes_dryguard_and_answers_its_own_source() {
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path = repository.join("src/lsp.rs");
    let source = source_of(&path).expect("自分のソースを読める");
    let root = WorkspaceRoot::enclosing(&[path.clone(), repository.join("Cargo.toml")])
        .expect("Cargo.toml を含む根");
    let document = SourceDocument::new(&path, source.clone()).expect("ソースを開ける");

    let started = Instant::now();
    let client = Client::start(&ServerCommand::rust()).expect("rust-analyzer を起動できる");
    let mut session = client.handshake(&root).expect("握手できる");
    let handshake = started.elapsed();
    session.open_document(&document).expect("ソースを開かせる");

    for name in ["typescript", "rust"] {
        let declaration = format!("pub fn {name}()");
        let line = source
            .lines()
            .position(|line| line.trim_start().starts_with(&declaration))
            .expect("関数宣言がある");
        let position =
            SourcePosition::from_preceding_text(LineNumber::from_index(line), "    pub fn ");

        let hover = session
            .hover(&document, position)
            .expect("hover を尋ねられる");
        assert!(
            matches!(hover, HoverOutcome::Answered(ref text) if text.as_str().contains(&declaration)),
            "{name} の宣言: {hover:?}"
        );

        let references = session
            .references(&document, position)
            .expect("references を尋ねられる");
        assert!(
            matches!(references, ReferencesOutcome::Answered(ref paths) if !paths.is_empty() && paths.iter().all(|path| path.starts_with(&repository))),
            "{name} の参照元: {references:?}"
        );
    }

    let queried = started.elapsed();
    session.shutdown().expect("正常終了できる");
    eprintln!(
        "dryguard repo: handshake={handshake:?}, indexing and hover/references x2={:?}, total={:?}",
        queried - handshake,
        started.elapsed()
    );
}

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_rust_scan_asks_candidates_in_one_session() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rust-lsp");
    let scan = scan_of_language(
        &root,
        SourceLanguage::Rust,
        TestFunctions::Excluded,
        ConfiguredThresholds::default(),
        &DomainDeclarations::default(),
        &ServerCommand::rust(),
    )
    .expect("Rust の候補を走査できる");

    assert!(!scan.candidate_pairs().is_empty(), "候補を見つける");
    assert!(
        scan.semantics_error().is_none(),
        "候補を揃えた後に 1 セッションで問い合わせる: {:?}",
        scan.semantics_error()
    );
    let text = scan_text_of(&scan, Explanation::AllSignals);
    assert!(
        text.contains("型シグネチャ: 単一化可能"),
        "引数名だけが違う i32 -> i32 の 2 つ: {text}"
    );
    assert!(
        text.contains("呼び出し元ドメインの重なり 1.00"),
        "references が返る: {text}"
    );
}

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_rust_analyzer_signatures_compare_through_where_clauses_and_lifetimes() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rust-bounds/src/lib.rs");
    let root = WorkspaceRoot::enclosing(std::slice::from_ref(&path)).expect("ワークスペースの根");
    let source = source_of(&path).expect("フィクスチャを読める");
    let document = SourceDocument::new(&path, source.clone()).expect("ドキュメントを作れる");

    let client = Client::start(&ServerCommand::rust()).expect("rust-analyzer を起動できる");
    let mut session = client.handshake(&root).expect("握手できる");
    session.open_document(&document).expect("ファイルを開ける");

    // pipeline と同じく、型名を辿った記録は渡さない
    let traced = TracedTypeNames::default();
    let mut outcome_of = |name: &str| {
        let line = source
            .lines()
            .position(|line| line.starts_with(&format!("pub fn {name}")))
            .expect("フィクスチャにその関数がある");
        let position = SourcePosition::from_preceding_text(LineNumber::from_index(line), "pub fn ");
        rust_type_signature_outcome_of(&mut session, &document, position, &traced)
            .expect("hover を尋ねられる")
    };

    let normalized = |outcome: TypeSignatureOutcome| match outcome {
        TypeSignatureOutcome::Normalized(signature) => signature,
        other => panic!("読み解けて比べられる: {other:?}"),
    };
    let first = normalized(outcome_of("first_len"));
    let second = normalized(outcome_of("second_len"));
    // hover は inline の境界を `where` へ寄せ、ライフタイムを書かれたとおりに返す
    let borrowed = normalized(outcome_of("borrowed_size"));
    let elided = normalized(outcome_of("elided_size"));
    let displayed = outcome_of("displayed_len");

    session.shutdown().expect("正常終了できる");

    assert!(
        first.is_unifiable_with(&second),
        "型変数名と引数名だけが違う"
    );
    assert!(
        borrowed.is_unifiable_with(&elided),
        "ライフタイムと境界の書き分けだけが違う"
    );
    assert!(!first.is_unifiable_with(&borrowed), "引数の型が違う");
    assert_eq!(
        displayed,
        TypeSignatureOutcome::UntracedTypeName {
            reason: UntracedReason::NoTracedRecord
        },
        "トレイトの名前はまだ辿らないので、綴りのまま比べない"
    );
}
