//! 実際の jdtls（Eclipse JDT Language Server）で hover / references / callHierarchy を確かめる。
//!
//! サーバを要するので通常の `cargo test` では飛ばし、CI で `--ignored` を実行する。

use std::path::PathBuf;
use std::time::Instant;

use dryguard::codebase::source_of;
use dryguard::line_number::LineNumber;
use dryguard::lsp::{
    CalleesOutcome, Client, HoverOutcome, ReferencesOutcome, ServerCommand, SourceDocument,
    WorkspaceRoot,
};
use dryguard::source_position::SourcePosition;

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
