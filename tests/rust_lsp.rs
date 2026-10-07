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
use dryguard::semantics::resolved_type::UnopenedReason;
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
        rust_type_signature_outcome_of(&mut session, &document, position, &type_references, None)
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
fn test_compare_rust_alias_is_opened_into_its_right_hand_side() {
    let charged = traced_fixture("src/lib.rs", "charged");
    let raw = traced_fixture("src/lib.rs", "raw");

    assert_eq!(
        traced_type_signature_match_of(&charged, &raw),
        TypeSignatureMatch::Unifiable
    );
}

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_compare_rust_imported_qualified_alias_is_unifiable_with_its_right_hand_side() {
    let alias = traced_fixture("src/lib.rs", "imported_alias");
    let raw = traced_fixture("src/lib.rs", "raw");
    assert_eq!(
        traced_type_signature_match_of(&alias, &raw),
        TypeSignatureMatch::Unifiable
    );
}

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_compare_rust_aliases_with_different_right_hand_sides_are_not_unifiable() {
    let large = traced_fixture("src/lib.rs", "imported_alias");
    let small = traced_fixture("src/lib.rs", "small_alias");
    assert_eq!(
        traced_type_signature_match_of(&large, &small),
        TypeSignatureMatch::NotUnifiable
    );
}

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_rust_aliases_with_evaluated_constant_lengths_are_unifiable() {
    let alias = traced_fixture("src/lib.rs", "counted_alias");
    assert_eq!(
        traced_type_signature_match_of(&alias, &alias),
        TypeSignatureMatch::Unifiable
    );
}

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_compare_rust_method_receiver_is_an_additional_typed_parameter() {
    let total = traced_fixture("src/lib.rs", "total");
    let charged = traced_fixture("src/lib.rs", "charged");
    assert_eq!(
        traced_type_signature_match_of(&total, &charged),
        TypeSignatureMatch::NotUnifiable
    );
}

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_compare_rust_methods_in_separate_impls_of_the_same_type_are_unifiable() {
    let first = traced_fixture("src/impls.rs", "borrow_a");
    let second = traced_fixture("src/impls.rs", "borrow_b");
    assert_eq!(
        traced_type_signature_match_of(&first, &second),
        TypeSignatureMatch::Unifiable
    );
}

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_rust_impl_signatures_preserve_target_receiver_bounds_and_binding_scopes() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rust-traced/src/impls.rs");
    let source = source_of(&path).expect("フィクスチャを読める");
    let tree = SyntaxTree::from_source(&source, Grammar::Rust).expect("木にできる");
    let document = SourceDocument::new(&path, source.clone()).expect("ドキュメントを作れる");
    let root = WorkspaceRoot::enclosing(std::slice::from_ref(&path)).expect("根がある");
    let mut session = Client::start(&ServerCommand::rust())
        .expect("サーバを起動できる")
        .handshake(&root)
        .expect("握手できる");
    session.open_document(&document).expect("開ける");
    let mut outcomes = std::collections::BTreeMap::new();
    for name in [
        "wrap_a",
        "wrap_b",
        "borrow_a",
        "borrow_b",
        "mutable",
        "owned",
        "owned_mut",
        "boxed",
        "explicit_box",
        "mixed_a",
        "mixed_b",
        "shadow",
        "target_shadow",
        "shadow_renamed",
        "other_bound",
        "different_target",
        "first_twin",
        "second_twin",
    ] {
        let location = traced_fixture("src/impls.rs", name);
        let chunk = Chunk::find_enclosing(&location, &tree).expect("メソッドがある");
        let outcome = rust_type_signature_outcome_of(
            &mut session,
            &document,
            chunk.name_position().expect("名前がある"),
            chunk.type_references(),
            chunk.rust_impl_header(),
        )
        .expect("問い合わせ成功");
        let TypeSignatureOutcome::Normalized(signature) = outcome else {
            panic!("{name}: {outcome:?}");
        };
        outcomes.insert(name, signature);
    }
    let access_line = source
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains("fn access"))
        .map(|(index, _)| index)
        .last()
        .expect("実装メソッドがある");
    let access_location = Location::new(path.clone(), LineNumber::from_index(access_line));
    let access = Chunk::find_enclosing(&access_location, &tree).expect("トレイトの実装メソッド");
    let access_outcome = rust_type_signature_outcome_of(
        &mut session,
        &document,
        access.name_position().expect("名前がある"),
        access.type_references(),
        access.rust_impl_header(),
    )
    .expect("問い合わせ成功");
    assert!(
        matches!(access_outcome, TypeSignatureOutcome::Normalized(_)),
        "{access_outcome:?}"
    );
    let projection_line = source
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains("fn projected"))
        .map(|(index, _)| index)
        .last()
        .expect("実装メソッドがある");
    let location = Location::new(path.clone(), LineNumber::from_index(projection_line));
    let projected = Chunk::find_enclosing(&location, &tree).expect("関連型を返すメソッド");
    let outcome = rust_type_signature_outcome_of(
        &mut session,
        &document,
        projected.name_position().expect("名前がある"),
        projected.type_references(),
        projected.rust_impl_header(),
    )
    .expect("問い合わせ成功");
    assert!(
        matches!(outcome, TypeSignatureOutcome::Normalized(_)),
        "{outcome:?}"
    );
    session.shutdown().expect("終了できる");
    for (first, second, expected) in [
        ("wrap_a", "wrap_b", true),
        ("borrow_a", "borrow_b", true),
        ("borrow_a", "mutable", false),
        ("borrow_a", "owned", false),
        ("owned", "owned_mut", true),
        ("boxed", "explicit_box", true),
        ("mixed_a", "mixed_b", false),
        ("shadow", "shadow_renamed", true),
        ("target_shadow", "shadow_renamed", true),
        ("borrow_a", "other_bound", false),
        ("borrow_a", "different_target", false),
        ("first_twin", "second_twin", false),
    ] {
        assert_eq!(
            outcomes[first].is_unifiable_with(&outcomes[second]),
            expected,
            "{first} / {second}"
        );
    }
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
            chunk.rust_impl_header(),
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

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_rust_alias_shadowing_a_primitive_is_not_compared_as_that_primitive() {
    let raw = traced_fixture("src/lib.rs", "raw");
    for function in [
        "shadowed_alias",
        "shadowed_pair",
        "shadowed_by_alias",
        "renamed_primitive",
    ] {
        let alias = traced_fixture("src/lib.rs", function);
        assert_eq!(
            traced_type_signature_match_of(&alias, &raw),
            TypeSignatureMatch::NotUnifiable,
            "{function}"
        );
    }
}

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_rust_compound_aliases_are_unifiable_with_their_right_hand_sides() {
    let alias = traced_fixture("src/lib.rs", "compound_alias");
    let raw = traced_fixture("src/lib.rs", "compound_raw");
    assert_eq!(
        traced_type_signature_match_of(&alias, &raw),
        TypeSignatureMatch::Unifiable
    );
}

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_rust_generic_aliases_are_unifiable_with_their_instantiated_types() {
    for (alias, raw) in [
        ("pair", "tuple"),
        ("nested", "nested_tuple"),
        ("defaulted", "tuple"),
        ("partial", "large"),
        ("bounded", "tuple"),
        ("flipped", "flipped_tuple"),
        ("primitive_parameter", "tuple"),
        ("nested_default", "default_tuple"),
        ("qualified", "tuple"),
        ("named", "named_tuple"),
    ] {
        let a = traced_fixture("src/generic.rs", alias);
        let b = traced_fixture("src/generic.rs", raw);
        assert_eq!(
            traced_type_signature_match_of(&a, &b),
            TypeSignatureMatch::Unifiable,
            "{alias} / {raw}"
        );
    }
}

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_rust_generic_alias_different_arguments_are_not_unifiable() {
    let a = traced_fixture("src/generic.rs", "pair");
    let b = traced_fixture("src/generic.rs", "large");
    assert_eq!(
        traced_type_signature_match_of(&a, &b),
        TypeSignatureMatch::NotUnifiable
    );
}

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_rust_generic_alias_unsupported_parameters_and_shadowed_defaults_are_unavailable() {
    for name in ["lifetime", "default_shadow"] {
        let alias = traced_fixture("src/generic.rs", name);
        assert_eq!(
            traced_type_signature_match_of(&alias, &alias),
            TypeSignatureMatch::UnopenedTypeName {
                reason: dryguard::semantics::resolved_type::UnopenedReason::UnopenableAlias,
            },
            "{name}"
        );
    }
}

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_rust_generic_alias_const_arguments_remain_unmeasurable() {
    let alias = traced_fixture("src/generic.rs", "constant");
    let outcome = traced_type_signature_match_of(&alias, &alias);
    assert!(
        matches!(
            outcome,
            TypeSignatureMatch::UnreadableSignature
                | TypeSignatureMatch::UnopenedTypeName {
                    reason: dryguard::semantics::resolved_type::UnopenedReason::UnopenableAlias,
                }
        ),
        "const の値を失った hover を綴りで比較しない: {outcome:?}"
    );
}

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_rust_alias_declaration_scope_matches_the_original_type() {
    let alias = traced_fixture("src/lib.rs", "named_alias");
    assert_eq!(
        traced_type_signature_match_of(&alias, &alias),
        TypeSignatureMatch::Unifiable
    );
}

#[test]
#[ignore = "rust-analyzer と rust-src が要る。CI では入れて --ignored で走らせる"]
fn test_rust_alias_chains_reexports_and_nested_types_keep_declaration_scope() {
    for (a, b) in [
        ("aliased", "direct"),
        ("chained", "direct"),
        ("imported", "direct"),
        ("paired", "pair_raw"),
        ("option", "option_raw"),
        ("instantiated", "pair_raw"),
        ("captured", "explicit"),
    ] {
        let first = traced_fixture("src/alias_scope.rs", a);
        let second = traced_fixture("src/alias_scope.rs", b);
        assert_eq!(
            traced_type_signature_match_of(&first, &second),
            TypeSignatureMatch::Unifiable,
            "{a} / {b}"
        );
    }
}

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_rust_aliases_do_not_capture_a_different_same_spelled_type() {
    let imported = traced_fixture("src/alias_scope.rs", "imported");
    let wrong = traced_fixture("src/alias_scope.rs", "wrong");
    assert_eq!(
        traced_type_signature_match_of(&imported, &wrong),
        TypeSignatureMatch::NotUnifiable
    );
}

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_rust_alias_same_spelled_types_from_different_modules_are_not_unifiable() {
    let a = traced_fixture("src/alias_scope.rs", "aliased");
    let b = traced_fixture("src/alias_scope.rs", "other_aliased");
    assert_eq!(
        traced_type_signature_match_of(&a, &b),
        TypeSignatureMatch::NotUnifiable
    );
}

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_rust_alias_cycles_and_expansion_limits_have_distinct_reasons() {
    use dryguard::semantics::resolved_type::UnopenedReason;
    for (name, reason) in [
        ("cyclic", UnopenedReason::CyclicAlias),
        ("limit", UnopenedReason::AliasExpansionLimit),
        ("size_limit", UnopenedReason::AliasExpansionLimit),
        ("warm_limit", UnopenedReason::AliasExpansionLimit),
        ("cold_limit", UnopenedReason::AliasExpansionLimit),
    ] {
        let a = traced_fixture("src/alias_scope.rs", name);
        assert_eq!(
            traced_type_signature_match_of(&a, &a),
            TypeSignatureMatch::UnopenedTypeName { reason },
            "{name}"
        );
    }
    let boundary = traced_fixture("src/alias_scope.rs", "boundary");
    let raw = traced_fixture("src/alias_scope.rs", "raw");
    assert_eq!(
        traced_type_signature_match_of(&boundary, &raw),
        TypeSignatureMatch::Unifiable
    );
}

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_rust_alias_same_spelled_constants_with_different_values_are_not_unifiable() {
    let a = traced_fixture("src/alias_scope.rs", "bytes_a");
    let b = traced_fixture("src/alias_scope.rs", "bytes_b");
    assert_eq!(
        traced_type_signature_match_of(&a, &b),
        TypeSignatureMatch::NotUnifiable
    );
}

#[test]
#[ignore = "rust-analyzer と rust-src が要る。CI では入れて --ignored で走らせる"]
fn test_rust_nominal_generic_arguments_are_preserved() {
    let a = traced_fixture("src/alias_scope.rs", "option_raw");
    let b = traced_fixture("src/alias_scope.rs", "other_option");
    assert_eq!(
        traced_type_signature_match_of(&a, &b),
        TypeSignatureMatch::NotUnifiable
    );
}

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_rust_shadowed_primitive_aliases_match_direct_uses() {
    let alias = traced_fixture("src/lib.rs", "shadowed_alias");
    let direct = traced_fixture("src/alias_scope.rs", "shadowed_direct");
    assert_eq!(
        traced_type_signature_match_of(&alias, &direct),
        TypeSignatureMatch::Unifiable
    );
    let raw = traced_fixture("src/lib.rs", "raw");
    assert_eq!(
        traced_type_signature_match_of(&direct, &raw),
        TypeSignatureMatch::NotUnifiable
    );
}

#[test]
#[ignore = "rust-analyzer と rust-src が要る。CI では入れて --ignored で走らせる"]
fn test_rust_nominal_generics_keep_lifetime_and_associated_binding_normalization() {
    for (a, b) in [
        ("lifetime_nominal", "lifetime_nominal_renamed"),
        ("iterator_binding", "iterator_binding_renamed"),
    ] {
        let first = traced_fixture("src/alias_scope.rs", a);
        let second = traced_fixture("src/alias_scope.rs", b);
        assert_eq!(
            traced_type_signature_match_of(&first, &second),
            TypeSignatureMatch::Unifiable,
            "{a} / {b}"
        );
    }
}

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_rust_associated_types_match_their_instantiated_right_hand_sides() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/rust-traced/src/projections.rs");
    let source = source_of(&path).expect("フィクスチャを読める");
    let tree = SyntaxTree::from_source(&source, Grammar::Rust).expect("木にできる");
    let document = SourceDocument::new(&path, source.clone()).expect("ドキュメントを作れる");
    let root = WorkspaceRoot::enclosing(std::slice::from_ref(&path)).expect("根がある");
    let mut session = Client::start(&ServerCommand::rust())
        .expect("サーバを起動できる")
        .handshake(&root)
        .expect("握手できる");
    session.open_document(&document).expect("開ける");
    let mut outcomes = std::collections::BTreeMap::new();
    for name in [
        "short",
        "qualified",
        "plain",
        "wrapped",
        "wrap_plain",
        "named",
        "named_plain",
        "other",
        "outside",
        "mismatch",
        "foreign",
        "foreign_plain",
        "outside_plain",
        "unsupported",
        "borrowed",
    ] {
        let index = source
            .lines()
            .enumerate()
            .filter(|(_, line)| {
                line.contains(&format!("fn {name}(")) || line.contains(&format!("fn {name}<"))
            })
            .map(|(index, _)| index)
            .last()
            .expect("メソッドがある");
        let location = Location::new(path.clone(), LineNumber::from_index(index));
        let chunk = Chunk::find_enclosing(&location, &tree).expect("チャンクがある");
        let outcome = rust_type_signature_outcome_of(
            &mut session,
            &document,
            chunk.name_position().expect("名前がある"),
            chunk.type_references(),
            chunk.rust_impl_header(),
        )
        .expect("問い合わせ成功");
        outcomes.insert(name, outcome);
    }
    session.shutdown().expect("終了できる");
    for (first, second) in [
        ("short", "plain"),
        ("qualified", "plain"),
        ("wrapped", "wrap_plain"),
        ("named", "named_plain"),
    ] {
        let TypeSignatureOutcome::Normalized(left) = &outcomes[first] else {
            panic!("{first}: {:?}", outcomes[first]);
        };
        let TypeSignatureOutcome::Normalized(right) = &outcomes[second] else {
            panic!("{second}: {:?}", outcomes[second]);
        };
        assert!(
            left.is_unifiable_with(right),
            "{first} / {second}: {left:?} / {right:?}"
        );
    }
    // 直接囲む impl とは別の trait（`Other`）の impl を選ぶ。`mismatch` は where 句の対照
    for (first, second) in [("foreign", "foreign_plain"), ("outside", "outside_plain")] {
        let TypeSignatureOutcome::Normalized(left) = &outcomes[first] else {
            panic!("{first}: {:?}", outcomes[first]);
        };
        let TypeSignatureOutcome::Normalized(right) = &outcomes[second] else {
            panic!("{second}: {:?}", outcomes[second]);
        };
        assert!(
            left.is_unifiable_with(right),
            "{first} / {second}: {left:?} / {right:?}"
        );
    }
    assert_eq!(
        outcomes["mismatch"],
        TypeSignatureOutcome::UnopenedTypeName {
            reason: UnopenedReason::UnresolvedAssociatedType
        }
    );
    for name in ["unsupported", "borrowed"] {
        assert_eq!(
            outcomes[name],
            TypeSignatureOutcome::UnopenedTypeName {
                reason: UnopenedReason::UnopenableAssociatedType
            },
            "{name}"
        );
    }
}

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_rust_qualified_self_projections_select_the_impl_matching_target_and_trait_arguments() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/rust-traced/src/selections.rs");
    let source = source_of(&path).expect("フィクスチャを読める");
    let tree = SyntaxTree::from_source(&source, Grammar::Rust).expect("木にできる");
    let document = SourceDocument::new(&path, source.clone()).expect("ドキュメントを作れる");
    let root = WorkspaceRoot::enclosing(std::slice::from_ref(&path)).expect("根がある");
    let mut session = Client::start(&ServerCommand::rust())
        .expect("サーバを起動できる")
        .handshake(&root)
        .expect("握手できる");
    session.open_document(&document).expect("開ける");
    let mut outcomes = std::collections::BTreeMap::new();
    for name in [
        "inherent",
        "inherent_plain",
        "sixteen",
        "sixteen_plain",
        "wrapped",
        "wrapped_plain",
        "renamed",
        "renamed_plain",
        "distant",
        "distant_plain",
        "shown",
        "shown_plain",
        "owned",
        "owned_plain",
        "bounded",
        "concrete",
        "blanket",
    ] {
        // trait の宣言（本体なし）より後ろにある実装メソッドを採る
        let index = source
            .lines()
            .enumerate()
            .filter(|(_, line)| {
                line.contains(&format!("fn {name}(")) || line.contains(&format!("fn {name}<"))
            })
            .map(|(index, _)| index)
            .last()
            .expect("メソッドがある");
        let location = Location::new(path.clone(), LineNumber::from_index(index));
        let chunk = Chunk::find_enclosing(&location, &tree).expect("チャンクがある");
        let outcome = rust_type_signature_outcome_of(
            &mut session,
            &document,
            chunk.name_position().expect("名前がある"),
            chunk.type_references(),
            chunk.rust_impl_header(),
        )
        .expect("問い合わせ成功");
        outcomes.insert(name, outcome);
    }
    session.shutdown().expect("終了できる");
    let normalized = |name: &str| {
        let TypeSignatureOutcome::Normalized(signature) = &outcomes[name] else {
            panic!("{name}: {:?}", outcomes[name]);
        };
        signature.clone()
    };
    for (first, second, expected) in [
        ("inherent", "inherent_plain", true),
        ("sixteen", "sixteen_plain", true),
        ("wrapped", "wrapped_plain", true),
        ("renamed", "renamed_plain", true),
        ("distant", "distant_plain", true),
        ("shown", "shown_plain", true),
        ("owned", "owned_plain", true),
        // 同じ trait の別の trait 引数の impl を選んでいない
        ("inherent", "sixteen_plain", false),
        // 同名の別 trait の impl を選んでいない
        ("renamed", "inherent_plain", false),
    ] {
        assert_eq!(
            normalized(first).is_unifiable_with(&normalized(second)),
            expected,
            "{first} / {second}"
        );
    }
    for name in ["bounded", "concrete", "blanket"] {
        assert_eq!(
            outcomes[name],
            TypeSignatureOutcome::UnopenedTypeName {
                reason: UnopenedReason::UnresolvedAssociatedType
            },
            "{name}"
        );
    }
}

#[test]
#[ignore = "rust-analyzer が要る。CI では入れて --ignored で走らせる"]
fn test_rust_array_lengths_compare_values_in_declaration_scope() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/rust-array-lengths/src/lib.rs");
    let source = source_of(&path).expect("フィクスチャを読める");
    let tree = SyntaxTree::from_source(&source, Grammar::Rust).expect("構文木を作れる");
    assert!(
        !tree.named_descendants()[0].has_error(),
        "usize の遮蔽を含め、フィクスチャに構文エラーがない"
    );
    let document = SourceDocument::new(&path, source.clone()).expect("ドキュメントを作れる");
    let root = WorkspaceRoot::enclosing(std::slice::from_ref(&path)).expect("根を作れる");
    let mut session = Client::start(&ServerCommand::rust())
        .expect("rust-analyzer を起動できる")
        .handshake(&root)
        .expect("握手できる");
    session.open_document(&document).expect("ソースを開ける");
    let mut outcomes = std::collections::BTreeMap::new();
    for (index, line) in source.lines().enumerate() {
        let Some(function) = line.trim_start().strip_prefix("pub fn ") else {
            continue;
        };
        let name = function.split(['(', '<']).next().expect("関数名");
        let location = Location::new(path.clone(), LineNumber::from_index(index));
        let chunk = Chunk::find_enclosing(&location, &tree).expect("関数のチャンク");
        let outcome = rust_type_signature_outcome_of(
            &mut session,
            &document,
            chunk.name_position().expect("関数の位置"),
            chunk.type_references(),
            chunk.rust_impl_header(),
        )
        .expect("LSP 問い合わせに成功する");
        outcomes.insert(name, outcome);
    }
    session.shutdown().expect("正常終了できる");

    let TypeSignatureOutcome::Normalized(literal) = &outcomes["literal"] else {
        panic!("{:?}", outcomes["literal"]);
    };
    for name in [
        "arithmetic",
        "imported",
        "aliased",
        "expression",
        "suffix",
        "commented",
        "scoped_count",
    ] {
        let TypeSignatureOutcome::Normalized(other) = &outcomes[name] else {
            panic!("{name}: {:?}", outcomes[name]);
        };
        assert!(literal.is_unifiable_with(other), "{name}: {other:?}");
    }
    let TypeSignatureOutcome::Normalized(different) = &outcomes["different"] else {
        panic!("{:?}", outcomes["different"]);
    };
    assert!(
        !literal.is_unifiable_with(different),
        "リテラルの長さ4と COUNT の値5は異なる"
    );
    let TypeSignatureOutcome::Normalized(scoped_count) = &outcomes["scoped_count"] else {
        panic!("{:?}", outcomes["scoped_count"]);
    };
    assert!(
        !scoped_count.is_unifiable_with(different),
        "同名 COUNT も宣言側の値4と5を区別する"
    );
    let TypeSignatureOutcome::Normalized(nested) = &outcomes["nested"] else {
        panic!("{:?}", outcomes["nested"]);
    };
    let TypeSignatureOutcome::Normalized(plain) = &outcomes["nested_plain"] else {
        panic!("{:?}", outcomes["nested_plain"]);
    };
    assert!(nested.is_unifiable_with(plain));
    let TypeSignatureOutcome::Normalized(different) = &outcomes["nested_different"] else {
        panic!("{:?}", outcomes["nested_different"]);
    };
    assert!(!nested.is_unifiable_with(different), "外側の長さも区別する");

    for (name, reason) in [
        ("unsupported", UnopenedReason::UnevaluableArrayLength),
        ("cyclic", UnopenedReason::CyclicArrayLength),
        ("limited", UnopenedReason::ArrayLengthEvaluationLimit),
        ("zero_division", UnopenedReason::UnevaluableArrayLength),
        ("negative", UnopenedReason::UnevaluableArrayLength),
        ("cast", UnopenedReason::UnevaluableArrayLength),
        ("shadowed", UnopenedReason::UnevaluableArrayLength),
        ("unsupported_alias", UnopenedReason::UnevaluableArrayLength),
        ("cyclic_alias", UnopenedReason::CyclicArrayLength),
        (
            "large_intermediate",
            UnopenedReason::ArrayLengthEvaluationLimit,
        ),
        ("chain_limit", UnopenedReason::ArrayLengthEvaluationLimit),
        ("work_limit", UnopenedReason::ArrayLengthEvaluationLimit),
        (
            "expression_limit",
            UnopenedReason::ArrayLengthEvaluationLimit,
        ),
        ("macro_type", UnopenedReason::UnmatchedArrayLength),
    ] {
        assert_eq!(
            outcomes[name],
            TypeSignatureOutcome::UnopenedTypeName { reason },
            "{name}"
        );
    }
    for name in ["maximum", "chain_boundary"] {
        assert!(
            matches!(outcomes[name], TypeSignatureOutcome::Normalized(_)),
            "{name}: {:?}",
            outcomes[name]
        );
    }
    assert_eq!(
        outcomes["generic"],
        TypeSignatureOutcome::UnreadableSignature
    );
}
