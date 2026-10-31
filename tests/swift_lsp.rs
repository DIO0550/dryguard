//! 実際の sourcekit-lsp で hover / references / callHierarchy を確かめる。
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
