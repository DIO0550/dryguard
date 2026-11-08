//! 実際の jdtls（Eclipse JDT Language Server）で hover / references / callHierarchy を確かめる。
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
    CalleesOutcome, Client, HoverOutcome, ReferencesOutcome, ServerCommand, SourceDocument,
    WorkspaceRoot,
};
use dryguard::pipeline::{chunk_pair_of, measured_pair_of};
use dryguard::source_position::SourcePosition;
use dryguard::threshold::Threshold;

#[test]
#[ignore = "jdtls が要る。CI では入れて --ignored で走らせる"]
fn test_jdtls_answers_hover_and_references_but_leaves_callees_unsettled() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/java-lsp");
    let path = fixture.join("src/main/java/shop/Total.java");
    let caller = fixture.join("src/main/java/shop/Checkout.java");
    // jdtls は根の pom.xml を Maven のプロジェクトとして取り込む（`pipeline` も印のディレクトリを根にする）
    let root = WorkspaceRoot::enclosing(&[path.clone(), fixture.join("pom.xml")])
        .expect("pom.xml を含む根");
    let document = SourceDocument::new(&path, source_of(&path).expect("フィクスチャを読める"))
        .expect("ドキュメントを作れる");
    let caller_document =
        SourceDocument::new(&caller, source_of(&caller).expect("フィクスチャを読める"))
            .expect("ドキュメントを作れる");

    let started = Instant::now();
    let client = Client::start(&ServerCommand::java()).expect("jdtls を起動できる");
    let mut session = client.handshake(&root).expect("握手できる");
    let handshake = started.elapsed();
    session.open_document(&document).expect("ファイルを開ける");

    for (line, name, spelling) in [
        (
            7,
            "total",
            "int shop.Total.total(List<Integer> items, double discount)",
        ),
        (
            12,
            "subtotal",
            "int shop.Total.subtotal(List<Integer> items, double rate)",
        ),
    ] {
        let position = SourcePosition::from_preceding_text(
            LineNumber::new(line).expect("正の行番号"),
            "    public static int ",
        );
        let hover = session
            .hover(&document, position)
            .expect("hover を尋ねられる");
        // jdtls 1.63.0 は `contents` を MarkedString の配列（`{language: "java", value: 綴り}`・Javadoc・
        // `Source:` の行）で返す。Javadoc を持つ `total` でも、綴りの要素だけを読む
        let HoverOutcome::Answered(text) = hover else {
            panic!("{name} の綴りが返る: {hover:?}");
        };
        assert_eq!(text.as_str(), spelling, "{name}");

        let references = session
            .references(&document, position)
            .expect("references を尋ねられる");
        let ReferencesOutcome::Answered(references) = references else {
            panic!("{name} の参照元が返る: {references:?}");
        };
        assert!(
            references
                .iter()
                .any(|reference| reference.path() == caller),
            "{name} の呼び出し元: {references:?}"
        );
    }
    let queried = started.elapsed();

    // 2 つの候補は JDK のメソッドだけを呼ぶ（jdtls はその outgoingCalls に空を返す）ので、
    // フィクスチャの中を呼ぶ側で尋ねる
    session
        .open_document(&caller_document)
        .expect("ファイルを開ける");
    let checkout = SourcePosition::from_preceding_text(
        LineNumber::new(6).expect("正の行番号"),
        "    public static int ",
    );
    let callees = session
        .callees(&caller_document, checkout)
        .expect("callHierarchy を尋ねられる");
    // jdtls は prepareCallHierarchy / outgoingCalls のたびに応答の前に作業（"Background task"）を
    // 始めて終えるので、作業に触れない答えが得られない
    assert!(
        matches!(callees, CalleesOutcome::ServerStillWorking),
        "checkout の呼び出し先: {callees:?}"
    );

    let called = started.elapsed();
    session.shutdown().expect("正常終了できる");
    // 終わった時点を 1 度だけ取る。呼ぶたびに取ると shutdown と total が別の時点を指す
    let total = started.elapsed();
    eprintln!(
        "jdtls: handshake={handshake:?}, open+hover/references x2={:?}, open+callHierarchy={:?}, shutdown={:?}, total={total:?}",
        queried - handshake,
        called - queried,
        total - called,
    );
}

/// `tests/fixtures/java-lsp` の `Signatures.java` の中で、`member` を宣言している行の位置。
///
/// `member` は宣言の名前と引数リストの開き（`added(`）。コンストラクタはクラス名で書く。
fn signature_fixture(member: &str) -> Location {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/java-lsp/src/main/java/shop/Signatures.java");
    let Ok(source) = source_of(&path) else {
        panic!("フィクスチャを読める: {}", path.display());
    };
    let declaration = format!(" {member}(");
    let Some(line) = source
        .lines()
        .position(|line| line.contains(&declaration) && line.trim_start().starts_with("public"))
    else {
        panic!("フィクスチャにその宣言がある: {member}");
    };

    let text = format!("{}:{}", path.display(), line + 1);
    let Ok(location) = text.parse() else {
        panic!("組み立てた位置は解釈できる: {text}");
    };
    location
}

/// 2 つのメンバーを `compare` の経路で比べた、型シグネチャのシグナル。
///
/// **構造類似度の閾値を 0 にして必ず尋ねる**（`tests/swift_lsp.rs` と同じ理由）。
fn type_signature_match_of(member_a: &str, member_b: &str) -> TypeSignatureMatch {
    let Ok(pair) = chunk_pair_of(&signature_fixture(member_a), &signature_fixture(member_b)) else {
        panic!("どちらもメンバーの中を指している");
    };
    let Some(every_pair) = Threshold::new(0.0) else {
        panic!("0.0 は閾値にできる");
    };
    let thresholds = ConfiguredThresholds::default().with_structural_similarity(every_pair);

    let Ok(measured) = measured_pair_of(
        &pair,
        thresholds,
        &DomainDeclarations::default(),
        &ServerCommand::java(),
    ) else {
        panic!("宣言が無ければ食い違わない");
    };
    if let Some(error) = measured.semantics_error() {
        panic!("実サーバには尋ねられる: {error}");
    }

    measured.signals().type_signature_match()
}

#[test]
#[ignore = "jdtls が要る。CI では入れて --ignored で走らせる"]
fn test_jdtls_methods_of_the_same_type_are_unifiable() {
    // メソッド名・引数名・本体だけが違う。型はプリミティブだけ
    assert_eq!(
        type_signature_match_of("added", "summed"),
        TypeSignatureMatch::Unifiable
    );
}

#[test]
#[ignore = "jdtls が要る。CI では入れて --ignored で走らせる"]
fn test_jdtls_methods_taking_different_types_are_not_unifiable() {
    assert_eq!(
        type_signature_match_of("added", "scaled"),
        TypeSignatureMatch::NotUnifiable
    );
}

#[test]
#[ignore = "jdtls が要る。CI では入れて --ignored で走らせる"]
fn test_jdtls_generic_methods_differing_only_in_type_parameter_names_are_unifiable() {
    // `<T extends Priced>` と `<U extends Priced>`。`Priced` は両側ともフィクスチャの同じ宣言へ辿る
    assert_eq!(
        type_signature_match_of("cheaper", "lower"),
        TypeSignatureMatch::Unifiable
    );
}

#[test]
#[ignore = "jdtls が要る。CI では入れて --ignored で走らせる"]
fn test_jdtls_type_parameters_with_different_bounds_are_not_unifiable() {
    // `<T extends Priced>` と `<T extends Named>`。形は同じで境界だけが違う
    assert_eq!(
        type_signature_match_of("cheaper", "earlier"),
        TypeSignatureMatch::NotUnifiable
    );
}

#[test]
#[ignore = "jdtls が要る。CI では入れて --ignored で走らせる"]
fn test_jdtls_constructor_and_instance_method_with_varargs_and_throws_are_read() {
    // 綴りを読めない側があれば単一化不能ではなく「測れない」になる。コンストラクタ・インスタンス
    // メソッド・可変長・`throws` の hover の綴りを読めることを、比べられた答えで確かめる
    assert_eq!(
        type_signature_match_of("Signatures", "offset"),
        TypeSignatureMatch::NotUnifiable
    );
}

#[test]
#[ignore = "jdtls が要る。CI では入れて --ignored で走らせる"]
fn test_jdtls_hover_spells_generic_constructor_and_throwing_members() {
    // 綴りの形の実測を CI のログに残す（`syntax::java_callable` が読む前提）
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/java-lsp");
    let path = fixture.join("src/main/java/shop/Signatures.java");
    let root = WorkspaceRoot::enclosing(&[path.clone(), fixture.join("pom.xml")])
        .expect("pom.xml を含む根");
    let source = source_of(&path).expect("フィクスチャを読める");
    let document = SourceDocument::new(&path, source.clone()).expect("ドキュメントを作れる");

    let client = Client::start(&ServerCommand::java()).expect("jdtls を起動できる");
    let mut session = client.handshake(&root).expect("握手できる");
    session.open_document(&document).expect("ファイルを開ける");

    for preceding in [
        "    public ",
        "    public static <T extends Priced> T ",
        "    public int ",
    ] {
        let Some(line) = source
            .lines()
            .position(|line| line.starts_with(preceding) && line.contains('('))
        else {
            panic!("フィクスチャにその行がある: {preceding}");
        };
        let position = SourcePosition::from_preceding_text(LineNumber::from_index(line), preceding);
        let hover = session
            .hover(&document, position)
            .expect("hover を尋ねられる");
        eprintln!("jdtls: {preceding:?}: hover={hover:?}");
        assert!(
            matches!(hover, HoverOutcome::Answered(_)),
            "{preceding}: 綴りが返る: {hover:?}"
        );
    }

    session.shutdown().expect("正常終了できる");
}
