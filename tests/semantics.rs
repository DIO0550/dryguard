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

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use dryguard::classification::signal::{CallerDomainOverlap, TypeSignatureMatch};
use dryguard::classification::verdict::Verdict;
use dryguard::classification::{DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD, classification_of};
use dryguard::codebase::source_of;
use dryguard::location::Location;
use dryguard::lsp::{
    Client, HoverOutcome, ReferencesOutcome, ServerCommand, Session, SourceDocument, WorkspaceRoot,
};
use dryguard::pipeline::{MeasuredPair, chunk_pair_of, measured_pair_of};
use dryguard::report::text_of;
use dryguard::semantics::caller_domain::CallerDomains;
use dryguard::semantics::resolved_type::ResolvedTypes;
use dryguard::semantics::type_signature::TypeSignature;
use dryguard::syntax::chunk::Chunk;

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
///
/// 根は `WorkspaceRoot::enclosing` が渡されたパスから決める。**テスト側で広げない**
/// （広げると、本番が作らない設定でテストが通る）。`tests/fixtures/references/` は
/// 候補ペアの共通の祖先（`src/`）に tsconfig.json を置いてあり、そこが根になる。
/// より下が根になるコードベースで呼び出し元が一部しか返らない話は Issue #125。
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
///
/// **型名は解決しない。** ここで見たいのは返った綴りを正規化して比べるところまでで、
/// 解決まで含めた形は `measured_with_an_lsp` を使うテストが見る。
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
    let Some(signature) =
        TypeSignature::from_signature_text(&signature_text, &ResolvedTypes::default())
    else {
        panic!("サーバが返した綴りは読み取れる: {signature_text}");
    };
    signature
}

/// 2 箇所のチャンクの型シグネチャが単一化できるか、実サーバに尋ねて確かめる。
fn unifiable(location_a: &Location, location_b: &Location) -> bool {
    let Ok(pair) = chunk_pair_of(location_a, location_b) else {
        panic!("テストが渡す位置はどちらも関数の中を指している");
    };

    let mut session = session_over(&[
        location_a.path().to_path_buf(),
        location_b.path().to_path_buf(),
    ]);
    let signature_a = type_signature_of(&mut session, pair.chunk_a());
    let signature_b = type_signature_of(&mut session, pair.chunk_b());
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

/// そのチャンクの呼び出し元のファイル。サーバに尋ねて集める。
fn reference_paths_of(session: &mut Session, chunk: &Chunk) -> Vec<PathBuf> {
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
    let outcome = session.references(&document, position);
    let Ok(ReferencesOutcome::Answered(reference_paths)) = outcome else {
        panic!(
            "名前の位置には参照元が返る: {} ({outcome:?})",
            chunk.path().display()
        );
    };
    reference_paths
}

/// そのチャンクの呼び出し元が属するドメイン。サーバに尋ねて数える。
fn caller_domains_of(session: &mut Session, chunk: &Chunk) -> CallerDomains {
    let reference_paths = reference_paths_of(session, chunk);

    let Some(caller_domains) = CallerDomains::from_reference_paths(&reference_paths) else {
        panic!("返った参照元は 1 件以上ある: {reference_paths:?}");
    };
    caller_domains
}

/// 参照元のファイル名（重複を畳んだもの）。**どのファイルが返ったか**を見るために使う。
fn reference_file_names(reference_paths: &[PathBuf]) -> BTreeSet<String> {
    reference_paths
        .iter()
        .filter_map(|path| path.file_name()?.to_str().map(str::to_owned))
        .collect()
}

/// 2 箇所のチャンクの呼び出し元ドメインがどれだけ重なるか、実サーバに尋ねて測る。
fn caller_domain_overlap(location_a: &Location, location_b: &Location) -> f64 {
    let Ok(pair) = chunk_pair_of(location_a, location_b) else {
        panic!("テストが渡す位置はどちらも関数の中を指している");
    };

    let mut session = session_over(&[
        location_a.path().to_path_buf(),
        location_b.path().to_path_buf(),
    ]);
    let domains_a = caller_domains_of(&mut session, pair.chunk_a());
    let domains_b = caller_domains_of(&mut session, pair.chunk_b());
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

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_caller_domains_asked_after_a_type_signature_are_still_complete() {
    // #29 が組む順（hover → references）を 1 つのセッションで通す。**先の hover で
    // サーバの作業を覚える**ので、その作業が終わる前に references を送ると、
    // 読み込み中に計算された答え（呼び出し元が欠けている）を受け取る
    let discounts_an_invoice = fixture("references/src/billing/discount.ts", 5);
    let reorders_stock = fixture("references/src/inventory/reorder.ts", 5);
    let Ok(pair) = chunk_pair_of(&discounts_an_invoice, &reorders_stock) else {
        panic!("テストが渡す位置はどちらも関数の中を指している");
    };

    // 候補ペアの 2 箇所だけを渡す。`compare` が本番で作る根と同じ決め方になる
    let mut session = session_over(&[
        discounts_an_invoice.path().to_path_buf(),
        reorders_stock.path().to_path_buf(),
    ]);
    let _signature = type_signature_of(&mut session, pair.chunk_a());
    let reference_paths = reference_paths_of(&mut session, pair.chunk_a());
    if session.shutdown().is_err() {
        panic!("サーバを終わらせられる");
    }

    // **ドメインに畳む前のファイルで見る。** 畳んでからだと、片方しか返らなくても
    // 「billing の 1 ドメイン」になり、欠けたことがテストから消える
    assert_eq!(
        reference_file_names(&reference_paths),
        BTreeSet::from(["invoice.ts".to_owned(), "statement.ts".to_owned()])
    );
}

/// 計画の出力イメージで `EXTRACT-CANDIDATE` 側に置かれているペア。
///
/// どちらも `(Date) => string` で、`utils/pad` に依存し、`report/monthly.ts` から呼ばれる。
fn shared_utility_pair() -> (Location, Location) {
    (
        fixture("references/src/utils/formatDate.ts", 3),
        fixture("references/src/report/dateHelper.ts", 3),
    )
}

/// 計画の出力イメージで `DO-NOT-EXTRACT` 側に置かれているペア。
///
/// 構造は同じだが `(Invoice) => number` と `(Stock) => number` で、呼び出し元も分かれる。
fn accidental_duplication_pair() -> (Location, Location) {
    (
        fixture("references/src/billing/discount.ts", 5),
        fixture("references/src/inventory/reorder.ts", 5),
    )
}

/// 2 箇所を切り出して、実サーバに Stage 2 を尋ねるところまで。
fn measured_with_an_lsp(location_a: &Location, location_b: &Location) -> MeasuredPair {
    let Ok(pair) = chunk_pair_of(location_a, location_b) else {
        panic!("テストが渡す位置はどちらも関数の中を指している");
    };

    let measured = measured_pair_of(
        &pair,
        DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD,
        &ServerCommand::typescript(),
    );
    if let Some(error) = measured.semantics_error() {
        panic!("実サーバには尋ねられる: {error}");
    }
    measured
}

/// 測れた呼び出し元ドメインの重なり。測れていなければ落とす。
fn caller_domain_overlap_value(measured: &MeasuredPair) -> f64 {
    let CallerDomainOverlap::Measured(callers) = measured.signals().caller_domain_overlap() else {
        panic!(
            "実サーバは参照元を返す: {:?}",
            measured.signals().caller_domain_overlap()
        );
    };

    callers.overlap().value()
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_finds_the_shared_utility_pair_unifiable() {
    let (formats_a_date, helps_with_dates) = shared_utility_pair();

    let measured = measured_with_an_lsp(&formats_a_date, &helps_with_dates);

    assert_eq!(
        measured.signals().type_signature_match(),
        TypeSignatureMatch::Unifiable
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_finds_the_accidental_duplication_not_unifiable() {
    // 対照は上のテスト。構造は似ているが、受け取る型が別ドメインのもの
    let (discounts_an_invoice, reorders_stock) = accidental_duplication_pair();

    let measured = measured_with_an_lsp(&discounts_an_invoice, &reorders_stock);

    assert_eq!(
        measured.signals().type_signature_match(),
        TypeSignatureMatch::NotUnifiable
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_opens_a_type_alias_written_on_a_parameter() {
    // hover が返すのは `function scaleAmount(amount: Amount, factor: number): Amount` で、
    // `Amount` は展開されない。解決しないと `(Amount, number) => Amount` と
    // `(number, number) => number` になり、単一化不能と出る
    let scales_an_amount = fixture("references/src/billing/scale.ts", 3);
    let scales_a_total = fixture("references/src/report/total.ts", 1);

    let measured = measured_with_an_lsp(&scales_an_amount, &scales_a_total);

    assert_eq!(
        measured.signals().type_signature_match(),
        TypeSignatureMatch::Unifiable
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_opens_a_type_alias_that_replaces_the_whole_signature() {
    // 呼び出し可能なエイリアスで注釈すると、hover は `const halveAmount: Scaling` と
    // 綴り全体をエイリアス名 1 語で返す。**引数リストが無いので、解決を綴りを読む前に
    // 差し込まないと入口に入れない**（`from_signature_text` が `None` を返す）
    let halves_an_amount = fixture("references/src/billing/scale.ts", 7);
    let halves_a_total = fixture("references/src/report/total.ts", 5);

    let measured = measured_with_an_lsp(&halves_an_amount, &halves_a_total);

    assert_eq!(
        measured.signals().type_signature_match(),
        TypeSignatureMatch::Unifiable
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_opens_a_type_alias_declared_outside_the_pair_root() {
    // 候補ペアが同じディレクトリにあると、根はそのディレクトリになる。エイリアスは
    // 兄弟ディレクトリで宣言されているので、**根の下だけを開かせる形では解決できない**
    let scales_by_rate = fixture("references/src/report/scaled.ts", 3);
    let scales_by_number = fixture("references/src/report/scaled.ts", 7);

    let measured = measured_with_an_lsp(&scales_by_rate, &scales_by_number);

    assert_eq!(
        measured.signals().type_signature_match(),
        TypeSignatureMatch::Unifiable
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_opens_a_type_alias_declared_by_a_dependency() {
    // `PropertyKey` は `lib.es5.d.ts`（約 1 MB・依存の置き場の下）で
    // `string | number | symbol` として宣言されている。開かせる相手を絞ると、
    // 書き下した綴りと比べる側が解決できない
    let keyed_by_property = fixture("references/src/report/keyed.ts", 1);
    let keyed_by_union = fixture("references/src/report/keyed.ts", 5);

    let measured = measured_with_an_lsp(&keyed_by_property, &keyed_by_union);

    assert_eq!(
        measured.signals().type_signature_match(),
        TypeSignatureMatch::Unifiable
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_does_not_unify_two_aliases_of_different_types_spelled_alike() {
    // どちらのファイルも自分だけの `Local` を宣言していて、中身は別物
    // （`{ amount: number }` と `{ label: string }`）。**`interface` は hover が
    // 構造を展開しない**ので、宣言の綴りは両側とも `type ... = Local` になる
    let boxed = fixture("references/src/billing/boxed.ts", 7);
    let wrapped = fixture("references/src/inventory/boxed.ts", 7);

    let measured = measured_with_an_lsp(&boxed, &wrapped);

    assert_eq!(
        measured.signals().type_signature_match(),
        TypeSignatureMatch::NotUnifiable
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_does_not_unify_two_generic_aliases_of_different_types_spelled_alike() {
    // 対照は 1 つ上のテスト。**総称型の参照でも同じことが起きる**。どちらのファイルも
    // 自分だけの `interface Local<T>` を宣言していて、開いた綴りは両側とも `Local<string>`
    let charged = fixture("references/src/billing/generic.ts", 7);
    let tagged = fixture("references/src/inventory/generic.ts", 8);

    let measured = measured_with_an_lsp(&charged, &tagged);

    assert_eq!(
        measured.signals().type_signature_match(),
        TypeSignatureMatch::NotUnifiable
    );
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_measures_a_total_caller_domain_overlap_for_the_shared_utility_pair() {
    // ディレクトリは utils と report で分かれているが、どちらも report/monthly.ts から
    // 呼ばれている。**置き場所ではなく実際に誰が使っているか**を見ているのがここ
    let (formats_a_date, helps_with_dates) = shared_utility_pair();

    let measured = measured_with_an_lsp(&formats_a_date, &helps_with_dates);

    assert_eq!(caller_domain_overlap_value(&measured), 1.0);
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_measures_no_caller_domain_overlap_for_the_accidental_duplication() {
    // 対照は上のテスト。applyDiscount は billing から、reorderAmount は inventory から呼ばれる
    let (discounts_an_invoice, reorders_stock) = accidental_duplication_pair();

    let measured = measured_with_an_lsp(&discounts_an_invoice, &reorders_stock);

    assert_eq!(caller_domain_overlap_value(&measured), 0.0);
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_keeps_the_accidental_duplication_a_do_not_extract() {
    let (discounts_an_invoice, reorders_stock) = accidental_duplication_pair();

    let measured = measured_with_an_lsp(&discounts_an_invoice, &reorders_stock);

    let classification =
        classification_of(measured.signals(), DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);
    assert_eq!(classification.verdict(), Verdict::DoNotExtract);
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_keeps_the_shared_utility_pair_an_extract_candidate() {
    // 対照は上のテスト。型シグネチャの拒否権が候補側を落としていないことも、ここで見る
    let (formats_a_date, helps_with_dates) = shared_utility_pair();

    let measured = measured_with_an_lsp(&formats_a_date, &helps_with_dates);

    let classification =
        classification_of(measured.signals(), DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);
    assert_eq!(classification.verdict(), Verdict::ExtractCandidate);
}

#[test]
#[ignore = "typescript-language-server が要る。CI では入れて --ignored で走らせる"]
fn test_compare_with_an_lsp_reports_the_stage2_signals_it_measured() {
    // 同じペアを LSP 無しで通したときは「測れない (LSP サーバを使えない)」が出る
    // （`tests/compare.rs`）。**判定に使われたシグナルの違いが出力から読める**のがこの Issue の完了条件
    let (discounts_an_invoice, reorders_stock) = accidental_duplication_pair();
    let measured = measured_with_an_lsp(&discounts_an_invoice, &reorders_stock);
    let classification =
        classification_of(measured.signals(), DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

    let text = text_of(
        &discounts_an_invoice,
        &reorders_stock,
        &classification,
        DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD,
    );

    assert!(
        text.contains("型シグネチャ: 単一化不能 → 共通化しない側"),
        "測った型シグネチャが値として出る: {text}"
    );
    assert!(
        text.contains("呼び出し元ドメインの重なり 0.00 (")
            && text.contains("billing 4件 <-> ")
            && text.contains("inventory 2件) → 共通化しない側"),
        "測った重なりと両側の分布が出る: {text}"
    );
}
