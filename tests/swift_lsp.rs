//! 実際の sourcekit-lsp で hover / references / callHierarchy を確かめる。
//!
//! サーバを要するので通常の `cargo test` では飛ばし、CI で `--ignored` を実行する。

use std::path::PathBuf;
use std::time::Instant;

use dryguard::classification::ConfiguredThresholds;
use dryguard::classification::signal::TypeSignatureMatch;
use dryguard::codebase::source_of;
use dryguard::domain_declaration::DomainDeclarations;
use dryguard::line_number::LineNumber;
use dryguard::location::Location;
use dryguard::lsp::{
    CalleesOutcome, Client, DeclarationSiteOutcome, HoverOutcome, ReferencesOutcome, ServerCommand,
    SourceDocument, WorkspaceRoot,
};
use dryguard::pipeline::{chunk_pair_of, measured_pair_of};
use dryguard::source_position::SourcePosition;
use dryguard::threshold::Threshold;

#[test]
#[ignore = "sourcekit-lsp が要る。CI では入れて --ignored で走らせる"]
fn test_sourcekit_lsp_answers_both_candidates_in_one_session() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/swift-lsp");
    let path = fixture.join("Sources/Shop/Total.swift");
    let caller = fixture.join("Sources/Shop/Checkout.swift");
    // sourcekit-lsp は根の Package.swift からビルド設定を組む。開くファイルのディレクトリを渡すと
    // index が作られず、別ファイルの参照元が返らない（`pipeline` も印のディレクトリを根にする）
    let root = WorkspaceRoot::enclosing(&[path.clone(), fixture.join("Package.swift")])
        .expect("Package.swift を含む根");
    let document = SourceDocument::new(&path, source_of(&path).expect("フィクスチャを読める"))
        .expect("ドキュメントを作れる");

    let started = Instant::now();
    let client = Client::start(&ServerCommand::swift()).expect("sourcekit-lsp を起動できる");
    let mut session = client.handshake(&root).expect("握手できる");
    let handshake = started.elapsed();
    session.open_document(&document).expect("ファイルを開ける");

    for (line, signature) in [
        (4, "func total(items: [Int], discount: Double = 0.1) -> Int"),
        (9, "func subtotal(items: [Int], rate: Double = 0.2) -> Int"),
    ] {
        let position = SourcePosition::from_preceding_text(
            LineNumber::new(line).expect("正の行番号"),
            "func ",
        );
        let hover = session
            .hover(&document, position)
            .expect("hover を尋ねられる");
        // 宣言に書かれたとおりの綴り（既定値を含む）を先頭のフェンスに置き、doc コメントはフェンスの外に続ける
        assert!(
            matches!(hover, HoverOutcome::Answered(ref text) if text.as_str() == signature),
            "{signature}: {hover:?}"
        );

        // swift build を走らせていなくても、background indexing が終われば別ファイルの参照元が返る
        let references = session
            .references(&document, position)
            .expect("references を尋ねられる");
        let ReferencesOutcome::Answered(references) = references else {
            panic!("{signature} の参照元が返る: {references:?}");
        };
        assert!(
            references
                .iter()
                .any(|reference| reference.path() == caller),
            "{signature} の呼び出し元: {references:?}"
        );

        let callees = session
            .callees(&document, position)
            .expect("callHierarchy を尋ねられる");
        assert!(
            matches!(callees, CalleesOutcome::Answered(ref paths) if !paths.is_empty()),
            "{signature} の呼び出し先: {callees:?}"
        );
    }

    let queried = started.elapsed();
    session.shutdown().expect("正常終了できる");
    // 握手の時間は index を作り終えるまでの待ちを含む
    eprintln!(
        "sourcekit-lsp: handshake+index={handshake:?}, hover/references/callHierarchy x2={:?}, shutdown={:?}, total={:?}",
        queried - handshake,
        started.elapsed() - queried,
        started.elapsed()
    );
}

/// `tests/fixtures/swift-lsp` の中で、`function` を宣言している行の位置。
fn signature_fixture(function: &str) -> Location {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/swift-lsp/Sources/Shop/Signatures.swift");
    let Ok(source) = source_of(&path) else {
        panic!("フィクスチャを読める: {}", path.display());
    };
    let declaration = format!("func {function}(");
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
/// **構造類似度の閾値を 0 にして必ず尋ねる**（`tests/rust_lsp.rs` と同じ理由）。
fn type_signature_match_of(function_a: &str, function_b: &str) -> TypeSignatureMatch {
    let Ok(pair) = chunk_pair_of(
        &signature_fixture(function_a),
        &signature_fixture(function_b),
    ) else {
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
        &ServerCommand::swift(),
    ) else {
        panic!("宣言が無ければ食い違わない");
    };
    if let Some(error) = measured.semantics_error() {
        panic!("実サーバには尋ねられる: {error}");
    }

    measured.signals().type_signature_match()
}

#[test]
#[ignore = "sourcekit-lsp が要る。CI では入れて --ignored で走らせる"]
fn test_sourcekit_lsp_functions_of_the_same_type_are_unifiable() {
    // 関数名と本体だけが違う。`Int` は両側とも標準ライブラリの同じ宣言へ辿る
    assert_eq!(
        type_signature_match_of("doubled", "tripled"),
        TypeSignatureMatch::Unifiable
    );
}

#[test]
#[ignore = "sourcekit-lsp が要る。CI では入れて --ignored で走らせる"]
fn test_sourcekit_lsp_functions_differing_only_in_labels_are_not_unifiable() {
    // `doubled(value: 1)` と `scaled(by: 1)`。内部名は同じ `value`
    assert_eq!(
        type_signature_match_of("doubled", "scaled"),
        TypeSignatureMatch::NotUnifiable
    );
}

#[test]
#[ignore = "sourcekit-lsp が要る。CI では入れて --ignored で走らせる"]
fn test_sourcekit_lsp_functions_taking_different_types_are_not_unifiable() {
    assert_eq!(
        type_signature_match_of("doubled", "halved"),
        TypeSignatureMatch::NotUnifiable
    );
}

#[test]
#[ignore = "sourcekit-lsp が要る。CI では入れて --ignored で走らせる"]
fn test_sourcekit_lsp_optional_sugar_and_named_optional_are_unifiable() {
    // hover は `Int?` と `Optional<Int>` を書かれたとおりに綴る。`Optional` が標準ライブラリを
    // 指すと辿れたときだけ、省略形と重ねる
    assert_eq!(
        type_signature_match_of("unwrapped", "defaulted"),
        TypeSignatureMatch::Unifiable
    );
}

#[test]
#[ignore = "sourcekit-lsp が要る。CI では入れて --ignored で走らせる"]
fn test_sourcekit_lsp_definition_of_standard_types_points_at_a_swift_interface() {
    // 標準ライブラリを宣言の場所で見分ける前提（`semantics::resolved_type` の
    // `is_swift_standard_library_site`）を、実サーバの答えで確かめる
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/swift-lsp");
    let path = fixture.join("Sources/Shop/Signatures.swift");
    let root = WorkspaceRoot::enclosing(&[path.clone(), fixture.join("Package.swift")])
        .expect("Package.swift を含む根");
    let source = source_of(&path).expect("フィクスチャを読める");
    let document = SourceDocument::new(&path, source.clone()).expect("ドキュメントを作れる");

    let client = Client::start(&ServerCommand::swift()).expect("sourcekit-lsp を起動できる");
    let mut session = client.handshake(&root).expect("握手できる");
    session.open_document(&document).expect("ファイルを開ける");

    let line_of = |text: &str| {
        source
            .lines()
            .position(|line| line.contains(text))
            .expect("フィクスチャにその行がある")
    };
    for (line_text, preceding) in [
        ("func doubled(", "func doubled(value: "),
        ("func defaulted(", "func defaulted(value: "),
    ] {
        let position = SourcePosition::from_preceding_text(
            LineNumber::from_index(line_of(line_text)),
            preceding,
        );
        let definition = session
            .definition(&document, position)
            .expect("definition を尋ねられる");
        let hover = session
            .hover(&document, position)
            .expect("hover を尋ねられる");
        eprintln!("sourcekit-lsp: {preceding}: definition={definition:?} hover={hover:?}");
        let DeclarationSiteOutcome::Answered(site) = definition else {
            panic!("{preceding}: 標準ライブラリの宣言の場所が返る: {definition:?} / {hover:?}");
        };
        let shown = site.path().display().to_string();
        assert!(
            shown.ends_with(".swiftinterface") && shown.contains("Swift"),
            "{preceding}: {shown} / {hover:?}"
        );
    }

    session.shutdown().expect("正常終了できる");
}
