//! 実際の rust-analyzer で hover / references を確かめる。
//!
//! サーバを要するので通常の `cargo test` では飛ばし、CI で `--ignored` を実行する。

use std::path::PathBuf;
use std::time::Instant;

use dryguard::classification::ConfiguredThresholds;
use dryguard::classification::signal::TypeSignatureMatch;
use dryguard::codebase::SourceLanguage;
use dryguard::codebase::source_of;
use dryguard::domain_declaration::DomainDeclarations;
use dryguard::line_number::LineNumber;
use dryguard::location::Location;
use dryguard::lsp::{
    Client, HoverOutcome, ReferencesOutcome, ServerCommand, SourceDocument, WorkspaceRoot,
};
use dryguard::pipeline::{chunk_pair_of, measured_pair_of, scan_of_language};
use dryguard::report::{Explanation, scan_text_of};
use dryguard::semantics::type_signature::{
    TypeSignatureOutcome, UntracedReason, rust_type_signature_outcome_of,
};
use dryguard::source_position::SourcePosition;
use dryguard::syntax::chunk::{Chunk, TestFunctions};
use dryguard::syntax::tree::{Grammar, SyntaxTree};
use dryguard::threshold::Threshold;

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
        assert!(
            paths.iter().any(|reference| reference.path() == path),
            "{name} の呼び出し元: {paths:?}"
        );
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
            matches!(references, ReferencesOutcome::Answered(ref paths) if !paths.is_empty() && paths.iter().all(|path| path.path().starts_with(&repository))),
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

    // 型名を渡さない（辿った記録が無い）。比べ方（境界・ライフタイムの書き分け）だけを見る
    let type_references = [];
    let mut outcome_of = |name: &str| {
        let line = source
            .lines()
            .position(|line| line.starts_with(&format!("pub fn {name}")))
            .expect("フィクスチャにその関数がある");
        let position = SourcePosition::from_preceding_text(LineNumber::from_index(line), "pub fn ");
        rust_type_signature_outcome_of(&mut session, &document, position, &type_references)
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
        "辿った記録が無ければ、トレイトの名前を綴りのまま比べない"
    );
}

/// `tests/fixtures/rust-traced` の中で、`function` を宣言している行の位置。
fn traced_fixture(relative_path: &str, function: &str) -> Location {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/rust-traced")
        .join(relative_path);
    let Ok(source) = source_of(&path) else {
        panic!("フィクスチャを読める: {}", path.display());
    };
    let declaration = format!("fn {function}");
    let Some(line) = source.lines().position(|line| line.contains(&declaration)) else {
        panic!("フィクスチャにその関数がある: {function}");
    };

    let text = format!("{}:{}", path.display(), line + 1);
    let Ok(location) = text.parse() else {
        panic!("組み立てた位置は解釈できる: {text}");
    };
    location
}

/// 2 つの関数を `compare` の経路で比べた、型シグネチャのシグナル。
///
/// **構造類似度の閾値を 0 にして必ず尋ねる。** 見たいのは型シグネチャの答えで、
/// 候補ペアかどうかで尋ねるかが変わると、本体の違いがシグナルの有無に混ざる。
fn traced_type_signature_match_of(
    location_a: &Location,
    location_b: &Location,
) -> TypeSignatureMatch {
    let Ok(pair) = chunk_pair_of(location_a, location_b) else {
        panic!("どちらも関数の中を指している");
    };
    let Some(every_pair) = Threshold::new(0.0) else {
        panic!("0.0 は閾値にできる");
    };
    let thresholds = ConfiguredThresholds::default().with_structural_similarity(every_pair);

    let Ok(measured) = measured_pair_of(
        &pair,
        thresholds,
        &DomainDeclarations::default(),
        &ServerCommand::rust(),
    ) else {
        panic!("宣言が無ければ食い違わない");
    };
    if let Some(error) = measured.semantics_error() {
        panic!("実サーバには尋ねられる: {error}");
    }

    measured.signals().type_signature_match()
}

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_compare_rust_functions_requiring_different_traits_are_not_unifiable() {
    // #40 の例をフィクスチャの中のトレイトで作る。辿った記録が渡るので、
    // 「尋ねていない」ではなく単一化不能と言い切れる
    let audited = traced_fixture("src/lib.rs", "audited_len");
    let exported = traced_fixture("src/lib.rs", "exported_len");

    assert_eq!(
        traced_type_signature_match_of(&audited, &exported),
        TypeSignatureMatch::NotUnifiable
    );
}

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_compare_rust_functions_requiring_the_same_trait_are_unifiable() {
    // 対照は上のテスト。型変数名・引数名・境界の書き場所（inline と where）だけが違う
    let audited_len = traced_fixture("src/lib.rs", "audited_len");
    let audited_count = traced_fixture("src/lib.rs", "audited_count");

    assert_eq!(
        traced_type_signature_match_of(&audited_len, &audited_count),
        TypeSignatureMatch::Unifiable
    );
}

#[test]
#[ignore = "rust-analyzer と rust-src が要る。CI では入れて --ignored で走らせる"]
fn test_compare_rust_functions_requiring_a_std_trait_are_unifiable() {
    // std の `Display` は rust-src が無いと宣言の場所が返らず、測れないに倒れる
    let displayed = traced_fixture("src/lib.rs", "displayed_len");
    let shown = traced_fixture("src/lib.rs", "shown_count");

    assert_eq!(
        traced_type_signature_match_of(&displayed, &shown),
        TypeSignatureMatch::Unifiable,
        "std の宣言まで辿れる（rust-src が入っているか確かめる）"
    );
}

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_compare_rust_functions_taking_same_spelled_types_of_different_modules_are_not_unifiable() {
    // どちらの hover も `User` と綴る。綴りのまま比べると単一化可能に出る（偽陽性）
    let billing = traced_fixture("src/billing.rs", "named");
    let inventory = traced_fixture("src/inventory.rs", "named");

    assert_eq!(
        traced_type_signature_match_of(&billing, &inventory),
        TypeSignatureMatch::NotUnifiable
    );
}

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_compare_rust_functions_taking_the_same_imported_type_from_different_files_are_unifiable() {
    // 対照は上のテスト。別々のファイルから同じ `crate::model::Customer` を指すと、
    // 宣言の場所が一致する
    let billing = traced_fixture("src/billing.rs", "greeted");
    let inventory = traced_fixture("src/inventory.rs", "greeted");

    assert_eq!(
        traced_type_signature_match_of(&billing, &inventory),
        TypeSignatureMatch::Unifiable
    );
}

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_compare_rust_functions_taking_the_same_alias_are_unifiable() {
    // `type Amount = u64` は typeDefinition だと空が返る。definition で辿るので記録が残る
    let charged = traced_fixture("src/lib.rs", "charged");
    let billed = traced_fixture("src/lib.rs", "billed");

    assert_eq!(
        traced_type_signature_match_of(&charged, &billed),
        TypeSignatureMatch::Unifiable
    );
}

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_compare_rust_alias_is_not_opened_into_its_right_hand_side() {
    // エイリアスは開かない（右辺の差し込みは別の Issue）。`Amount` と `u64` は
    // 同じ型だが重ならない。倒れる向きは偽陰性
    let charged = traced_fixture("src/lib.rs", "charged");
    let raw = traced_fixture("src/lib.rs", "raw");

    assert_eq!(
        traced_type_signature_match_of(&charged, &raw),
        TypeSignatureMatch::NotUnifiable
    );
}

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_compare_rust_method_with_a_receiver_stays_site_dependent() {
    // 辿った記録が渡るようになっても、`&self` を持つ側を比べられる答えにしない
    // （指す先は囲む `impl` で決まる）。**辿れたかどうかは見ていない** — 相手の `charged` が
    // 辿れず「尋ねていない」になっても、書かれた場所で決まる綴りのほうが先に出る
    // （pipeline の `test_type_signature_match_of_a_site_dependent_spelling_outranks_*`）
    let total = traced_fixture("src/lib.rs", "total");
    let charged = traced_fixture("src/lib.rs", "charged");

    assert_eq!(
        traced_type_signature_match_of(&total, &charged),
        TypeSignatureMatch::SiteDependentSpelling
    );
}

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_rust_type_references_of_a_chunk_are_the_type_names_of_its_hover() {
    // 集める側（ソース）と数える側（hover の綴り）で綴りが食い違うと、集めた記録を
    // 綴りで引けずに「尋ねていない」へ倒れる。実サーバの綴りで一致を確かめる
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rust-traced/src/lib.rs");
    let source = source_of(&path).expect("フィクスチャを読める");
    let root = WorkspaceRoot::enclosing(std::slice::from_ref(&path)).expect("ワークスペースの根");
    let document = SourceDocument::new(&path, source.clone()).expect("ドキュメントを作れる");
    let tree = SyntaxTree::from_source(&source, Grammar::Rust).expect("木にできる");

    let client = Client::start(&ServerCommand::rust()).expect("rust-analyzer を起動できる");
    let mut session = client.handshake(&root).expect("握手できる");
    session.open_document(&document).expect("ファイルを開ける");

    let mut outcomes = Vec::new();
    for function in ["audited_count", "displayed_len", "charged"] {
        let location = traced_fixture("src/lib.rs", function);
        let chunk = Chunk::find_enclosing(&location, &tree).expect("関数を切り出せる");
        let position = chunk.name_position().expect("名前がある");
        let outcome = rust_type_signature_outcome_of(
            &mut session,
            &document,
            position,
            chunk.type_references(),
        )
        .expect("hover と definition を尋ねられる");
        outcomes.push((function, outcome));
    }

    session.shutdown().expect("正常終了できる");

    for (function, outcome) in outcomes {
        assert!(
            matches!(outcome, TypeSignatureOutcome::Normalized(_)),
            "{function}: 比較に残る型名にすべて記録がある: {outcome:?}"
        );
    }
}

/// 実サーバが返した参照から、本番の件数だけが残ることを検証する。
fn assert_production_callers(
    signal: &dryguard::classification::signal::CallerDomainOverlap,
    source: &std::path::Path,
) {
    use dryguard::classification::signal::CallerDomainOverlap;
    use dryguard::semantics::domain::Domain;
    let CallerDomainOverlap::Measured(measured) = signal else {
        panic!("参照元を測れる: {signal:?}");
    };
    let Ok(domain) = Domain::of_path(source, &DomainDeclarations::default()) else {
        panic!("宣言がなければ曖昧にならない");
    };
    assert_eq!(
        measured.callers_a().references_per_domain(),
        vec![(&domain, 2)]
    );
    assert_eq!(
        measured.callers_b().references_per_domain(),
        vec![(&domain, 1)]
    );
}

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_rust_references_exclude_tests_in_compare_and_scan() {
    let root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rust-test-references");
    let path = root.join("src/lib.rs");
    let document = SourceDocument::new(&path, source_of(&path).expect("ソースを読める"))
        .expect("ドキュメントを作れる");
    let workspace =
        WorkspaceRoot::enclosing(std::slice::from_ref(&path)).expect("Cargo プロジェクト");
    let mut session = Client::start(&ServerCommand::rust())
        .expect("サーバを起動できる")
        .handshake(&workspace)
        .expect("握手できる");
    session
        .open_document(&document)
        .expect("ファイルを開かせる");
    let position =
        SourcePosition::from_preceding_text(LineNumber::new(3).expect("正の行"), "pub fn ");
    let raw = session
        .references(&document, position)
        .expect("参照を尋ねられる");
    let ReferencesOutcome::Answered(references) = raw else {
        panic!("参照が返る: {raw:?}");
    };
    assert!(
        references
            .iter()
            .any(|reference| reference.path() == path.with_file_name("report.rs")),
        "テスト専用ファイルからの参照もサーバは返す: {references:?}"
    );
    assert!(
        references
            .iter()
            .any(|reference| reference.path() == path && reference.position().line().get() == 17),
        "同一ファイルのテストからの参照も返す: {references:?}"
    );
    session.shutdown().expect("終了できる");

    let first = Location::new(path.clone(), LineNumber::new(3).expect("正の行"));
    let second = Location::new(path.clone(), LineNumber::new(7).expect("正の行"));
    let pair = chunk_pair_of(&first, &second).expect("本番関数のペア");
    let measured = measured_pair_of(
        &pair,
        ConfiguredThresholds::default(),
        &DomainDeclarations::default(),
        &ServerCommand::rust(),
    )
    .expect("宣言がない");
    assert_production_callers(measured.signals().caller_domain_overlap(), &path);

    let scan = scan_of_language(
        &root,
        SourceLanguage::Rust,
        TestFunctions::Included,
        ConfiguredThresholds::default(),
        &DomainDeclarations::default(),
        &ServerCommand::rust(),
    )
    .expect("走査できる");
    let pair = scan
        .candidate_pairs()
        .iter()
        .find(|pair| pair.location_a() == &first && pair.location_b() == &second)
        .expect("本番関数のペアが残る");
    let signal = pair
        .classification()
        .reasons()
        .iter()
        .find_map(|reason| {
            let dryguard::classification::reason::Reason::CallerDomainOverlap { signal, .. } =
                reason
            else {
                return None;
            };
            Some(signal)
        })
        .expect("呼び出し元の根拠がある");
    assert_production_callers(signal, &path);
}
