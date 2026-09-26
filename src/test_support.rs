//! テストだけが使う値の組み立て。
//!
//! 置いてよいのは、**テストであることに依存しているもの**に限る
//! (rules/testing.md「テスト用ヘルパーの置き場所」)。ここにあるのは
//! 「テストが渡す値は前提を満たしている」という表明を含むので、実装側には置けない。
//!
//! 汎用の操作（値を別の表現へ直すなど）は実装側の責務なので、ここに書かない。

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use crate::classification::ConfiguredThresholds;
use crate::domain_declaration::{DomainDeclaration, DomainDeclarations, DomainName};
use crate::line_number::LineNumber;
use crate::location::Location;
use crate::lsp::{DeclarationSite, ServerCommand, SignatureText};
use crate::pipeline::{Scan, scan_of};
use crate::source_position::SourcePosition;

/// このリポジトリの中のパス。
///
/// 実在するファイルを指す必要があるテスト（パスを辿る・URI にするなど）が使う。
/// カレントディレクトリではなくクレートの位置から組み立てるので、
/// どこから `cargo test` を呼んでも同じ場所を指す。
pub(crate) fn repository_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

/// 1 始まりの行番号。
///
/// # Panics
///
/// `number` が 0 のとき。テストが 0 行目を渡すのは、テスト自体の書き間違い。
pub(crate) fn line(number: usize) -> LineNumber {
    LineNumber::new(number).expect("テストが渡す行番号は 1 以上")
}

/// 起動できない LSP サーバの指定。
///
/// **モックではなく実物の失敗**を使う（`rules/testing.md`「モックは使わない」）。
/// 実行ファイルが無いので `Client::start` が `ServerNotFound` で落ち、
/// **サーバが入っている環境でも入っていない環境でも同じ経路を通る**。
pub(crate) fn missing_server() -> ServerCommand {
    ServerCommand::new("dryguard-no-such-language-server", Vec::new(), Vec::new())
}

/// hover が返した綴り。
///
/// # Panics
///
/// `text` が空白しか持たないとき。空の綴りは hover の結果として作れないので、
/// テストが渡すのは書き間違い。
pub(crate) fn signature_text(text: &str) -> SignatureText {
    SignatureText::new(text.to_owned()).expect("テストが渡す綴りは空ではない")
}

/// サーバが数えたオーバーロードの本数。
///
/// # Panics
///
/// `count` が 0 のとき。綴られている 1 本を数に含めるので 0 本にはならず、
/// テストが渡すのは書き間違い。
pub(crate) fn overload_count(count: usize) -> NonZeroUsize {
    NonZeroUsize::new(count).expect("テストが渡す本数は 1 以上")
}

/// 型が宣言されている場所。行の先頭を指す。
///
/// 実在しないパスでよい（`file:` URI にするだけで、開きはしない）。
///
/// # Panics
///
/// `path` が絶対パスでないとき。相対パスは `file:` URI にできないので、
/// テストが渡すのは書き間違い。
pub(crate) fn declaration_site(path: &str, number: usize) -> DeclarationSite {
    let position = SourcePosition::from_preceding_text(line(number), "");

    DeclarationSite::new(Path::new(path), position).expect("テストが渡すパスは絶対パス")
}

/// `/repo` を起点にした、ドメインの宣言の一覧。
///
/// `declared` は宣言の名前と、その glob の組。照合は綴りだけを見るので、
/// `/repo` は実在しなくてよい。
///
/// # Panics
///
/// 名前が裸のキーでない・glob が読めない・glob が 1 つも無いとき。
/// どれも設定の読み取りが `Err` にするので、テストが渡すのは書き間違い。
pub(crate) fn declarations_of(declared: &[(&str, &[&str])]) -> DomainDeclarations {
    let declarations = declared
        .iter()
        .map(|(name, patterns)| {
            DomainDeclaration::new(
                DomainName::new(name).expect("テストが渡す名前は裸のキー"),
                patterns
                    .iter()
                    .map(|pattern| pattern.parse().expect("テストが渡す glob は読める"))
                    .collect(),
            )
            .expect("テストが渡す宣言は glob を 1 つ以上持つ")
        })
        .collect();

    DomainDeclarations::new(Path::new("/repo"), declarations)
}

/// ファイルと行で表した位置。
///
/// 実在するファイルを指さなくてよい（綴りを出すだけのテストが使う）。
///
/// # Panics
///
/// `number` が 0 のとき。[`line`] と同じ理由。
pub(crate) fn location(path: &str, number: usize) -> Location {
    Location::new(PathBuf::from(path), line(number))
}

/// `tests/fixtures/` 配下のディレクトリを走査した結果。
///
/// カレントディレクトリではなくクレートの位置から組み立てるので、どこから
/// `cargo test` を呼んでも同じ場所を指す。
///
/// **起動できないサーバを渡す。** 実サーバを要する形にすると、サーバの入っていない
/// 開発機で出力が変わる（`rules/testing.md`「LSP を要するテストは、飛ばしたことが
/// 分かる形にする」）。
///
/// # Panics
///
/// 走査を始められないとき。フィクスチャのディレクトリは実在するので、
/// テストが渡すパスの書き間違い。
pub(crate) fn scan_of_fixture(relative_path: &str, thresholds: ConfiguredThresholds) -> Scan {
    let root = repository_path(&format!("tests/fixtures/{relative_path}"));

    scan_of(
        &root,
        thresholds,
        &DomainDeclarations::default(),
        &missing_server(),
    )
    .expect("フィクスチャのディレクトリは走査できる")
}
