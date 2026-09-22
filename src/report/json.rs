//! 判定と根拠を、エージェントが読む JSON にする。
//!
//! **判定はしない。** 親モジュールの text 化と同じく、ラベルと根拠を受け取って
//! 並べるだけで、シグナルからラベルを決めるのは `classification` にしか無い
//! （`rules/architecture.md`「判定は 1 箇所にだけ置く」）。
//!
//! **文にしない。** text 側が組み立てる日本語の文はここには出さず、シグナルの値・
//! 取れなかった理由・傾きを**キーと値のまま**出す（`rules/naming.md`「`reason` は
//! 文ではなく構造」）。文にしてから渡すと、読む側は判定に効いた値と向きを
//! 文字列から取り出し直すことになる。
//!
//! **text で出している情報を落とさない。** 理由に付く数値や綴り
//! （オーバーロードの本数・プロジェクトの印・読み取れなかった行）も、
//! 理由のキーと一緒に運ぶ。

use serde_json::{Map, Value, json};

use crate::classification::Classification;
use crate::classification::reason::{Lean, Reason};
use crate::classification::signal::{
    CallerDomainOverlap, ImportOverlap, MeasuredCallerDomains, SemanticsUnavailable,
    StructuralSimilarity, TypeSignatureMatch,
};
use crate::location::Location;
use crate::pipeline::{Scan, SkippedFile};
use crate::report::Explanation;
use crate::semantics::caller_domain::CallerDomains;
use crate::semantics::resolved_type::UnopenedReason;
use crate::semantics::type_signature::UntracedReason;
use crate::syntax::import::ImportsUnavailable;
use crate::syntax::module_distance::ModuleDistance;
use crate::threshold::Threshold;

/// シグナルの値が測れたときの `signal status`。
const MEASURED_STATUS: &str = "measured";

/// 尋ねたが測れなかったときの `signal status`。
const UNMEASURABLE_STATUS: &str = "unmeasurable";

/// そもそも尋ねていないときの `signal status`。
const NOT_ASKED_STATUS: &str = "not-asked";

/// 判定が当てた境目を載せるキー。
const THRESHOLD_KEY: &str = "threshold";

/// `signal status` を載せるキー。
const STATUS_KEY: &str = "status";

/// 測れなかった / 尋ねていない理由を載せるキー。
const REASON_KEY: &str = "reason";

/// 1 つのペアの判定と根拠を JSON にする。
///
/// `explanation` は根拠をどこまで出すか（`--explain` が [`Explanation::AllSignals`]）。
/// **text と同じ出し分けをする** — 尋ねなかったシグナルの項目と、当てた閾値は
/// `--explain` のときだけ出る。形式によって出る中身が変わると、
/// **同じ判定を 2 通りの範囲で読むことになる**。
///
/// 末尾に改行は付けない（呼ぶ側が `println!` で出す）。
pub fn json_of(
    location_a: &Location,
    location_b: &Location,
    classification: &Classification,
    explanation: Explanation,
) -> String {
    let value = pair_value_of(location_a, location_b, classification, explanation);

    format!("{value:#}")
}

/// 走査の結果を JSON にする。
///
/// `explanation` は根拠をどこまで出すか（`--explain` が [`Explanation::AllSignals`]）。
///
/// 候補ペアは [`json_of`] と同じ形で並べる。**`compare` と `scan` で同じペアの
/// 見え方が変わると、片方で見た結果をもう片方で確かめられない。**
///
/// 飛ばしたファイルと切り出せなかった関数は、**1 件も無くても配列を出す**。
/// text が節ごと消すのは見出しだけの節を読ませないためだが、JSON の読む側は
/// キーの有無で分岐するより空配列を受け取るほうが短い。
///
/// 末尾に改行は付けない（呼ぶ側が `println!` で出す）。
pub fn scan_json_of(scan: &Scan, explanation: Explanation) -> String {
    let value = scan_value_of(scan, explanation);

    format!("{value:#}")
}

/// 1 つのペアの判定・位置・根拠・提案。
///
/// **提案も出す。** ラベルから決まるので読む側でも導けるが、text に出ていて
/// JSON に無い項目を作らない（Issue #34「text で出している情報が JSON で落ちない
/// ようにする」）。組み立ては text 側と同じ [`super::suggestion_of`] を使い、
/// **形式ごとに別の文を持たない**。
fn pair_value_of(
    location_a: &Location,
    location_b: &Location,
    classification: &Classification,
    explanation: Explanation,
) -> Value {
    let reasons: Vec<Value> = classification
        .reasons()
        .iter()
        .filter_map(|reason| reason_value_of(reason, explanation))
        .collect();

    json!({
        "verdict": classification.verdict().to_string(),
        "location_a": location_value_of(location_a),
        "location_b": location_value_of(location_b),
        "reasons": reasons,
        "suggestion": super::suggestion_of(classification.verdict()),
    })
}

/// 走査した結果一式。
fn scan_value_of(scan: &Scan, explanation: Explanation) -> Value {
    let candidate_pairs: Vec<Value> = scan
        .candidate_pairs()
        .iter()
        .map(|pair| {
            pair_value_of(
                pair.location_a(),
                pair.location_b(),
                pair.classification(),
                explanation,
            )
        })
        .collect();
    let skipped_files: Vec<Value> = scan
        .skipped_files()
        .iter()
        .map(skipped_file_value_of)
        .collect();
    let unchunkable: Vec<Value> = scan.unchunkable().iter().map(location_value_of).collect();

    json!({
        "candidate_pairs": candidate_pairs,
        "skipped_files": skipped_files,
        "unchunkable": unchunkable,
        "walked": walked_value_of(scan),
    })
}

/// 走査した量。**候補の数だけでは「見ていないもの」が分からない。**
///
/// **候補ペアの数はここに入れない。** `candidate_pairs` の要素数がそのまま答えで、
/// 同じ数を 2 箇所に置くと片方だけ直したときに食い違う（text が併記しているのは、
/// 読む側が並んだペアを数え直さずに済ませるため）。
fn walked_value_of(scan: &Scan) -> Value {
    json!({
        "files": scan.file_count(),
        "chunks": scan.chunk_count(),
        "compared_pairs": scan.compared_pair_count(),
        "pruned_pairs": scan.pruned_pair_count(),
    })
}

/// ファイルと行。
fn location_value_of(location: &Location) -> Value {
    json!({
        "file": location.path().display().to_string(),
        "line": location.line().get(),
    })
}

/// 飛ばしたファイルと、その理由。
///
/// **`cause` を落とさない。** 読めなかった理由は OS やパーサが返した文でしか
/// 持っておらず、キーに畳むと text より情報が減る。
fn skipped_file_value_of(skipped: &SkippedFile) -> Value {
    match skipped {
        SkippedFile::UnreadableExtension { path } => json!({
            "file": path.display().to_string(),
            "reason": "unreadable-extension",
        }),
        SkippedFile::SourceUnreadable { path, cause } => json!({
            "file": path.display().to_string(),
            "reason": "source-unreadable",
            "cause": cause.to_string(),
        }),
        SkippedFile::SourceUnparsable { path, cause } => json!({
            "file": path.display().to_string(),
            "reason": "source-unparsable",
            "cause": cause.to_string(),
        }),
    }
}

/// 根拠 1 件。尋ねていないシグナルを既定で出さないときだけ `None`。
///
/// **当てた境目のキーはシグナルごとに選ぶ。** モジュール距離だけ `threshold` ではなく
/// `separate_directory_steps` にしてあるのは、**0.0-1.0 の閾値ではなく段数**だから
/// （同じキーに入れると、読む側は重なりの値として読む。text 側が
/// `applied_steps_text_of` を分けているのと同じ理由）。
fn reason_value_of(reason: &Reason, explanation: Explanation) -> Option<Value> {
    match reason {
        Reason::StructuralSimilarity {
            signal,
            threshold,
            lean,
        } => {
            let value = structural_similarity_value_of(*signal);
            let applied = measured_threshold_of(&value, *threshold);

            Some(signal_value_of(
                "structural-similarity",
                value,
                *lean,
                applied,
            ))
        }
        Reason::ImportOverlap {
            signal,
            threshold,
            lean,
        } => {
            let value = import_overlap_value_of(*signal);
            let applied = explained_threshold_of(&value, *threshold, explanation);

            Some(signal_value_of("import-overlap", value, *lean, applied))
        }
        Reason::ModuleDistance {
            signal,
            separate_directory_steps,
            lean,
        } => {
            let applied = explained_steps_of(*separate_directory_steps, explanation);

            Some(signal_value_of(
                "module-distance",
                module_distance_value_of(*signal),
                *lean,
                applied,
            ))
        }
        Reason::TypeSignatureMatch { signal, lean } => Some(signal_value_of(
            "type-signature-match",
            type_signature_value_of(*signal, explanation)?,
            *lean,
            None,
        )),
        Reason::CallerDomainOverlap {
            signal,
            threshold,
            lean,
        } => {
            let value = caller_domain_value_of(signal, explanation)?;
            let applied = explained_threshold_of(&value, *threshold, explanation);

            Some(signal_value_of(
                "caller-domain-overlap",
                value,
                *lean,
                applied,
            ))
        }
    }
}

/// どのシグナルが、どの値で、どちらへ傾けたか。
///
/// `applied` は判定が当てた境目（閾値・段数）。付ける条件はシグナルごとに違うので、
/// 呼ぶ側が決めてから渡す。
fn signal_value_of(name: &str, value: Value, lean: Lean, applied: Option<(&str, Value)>) -> Value {
    let mut object = Map::new();
    object.insert("signal".to_owned(), Value::from(name));
    object.insert("value".to_owned(), value);
    object.insert("lean".to_owned(), Value::from(lean_name_of(lean)));

    if let Some((key, applied_value)) = applied {
        object.insert(key.to_owned(), applied_value);
    }

    Value::Object(object)
}

/// 測れたシグナルに当てた閾値。**測れていなければ付けない。**
///
/// 比べていない値に閾値を並べると、比べた結果として読める（text 側の
/// `structural_similarity_text_of` と同じ扱い）。
///
/// **`--explain` を見ない。** `--threshold` で動く唯一の閾値が構造類似度なので、
/// 既定でも出さないと指定がどこに効いたのかを出力から読めない。
fn measured_threshold_of(value: &Value, threshold: Threshold) -> Option<(&'static str, Value)> {
    if value[STATUS_KEY] != MEASURED_STATUS {
        return None;
    }

    Some((THRESHOLD_KEY, json!(threshold.value())))
}

/// `--explain` のときだけ付ける、測れたシグナルに当てた閾値。
///
/// 既定で付けないのは、**ハードコードの閾値は出力を読む側が動かせない**から
/// （text 側の `applied_threshold_text_of` と同じ扱い）。
fn explained_threshold_of(
    value: &Value,
    threshold: Threshold,
    explanation: Explanation,
) -> Option<(&'static str, Value)> {
    match explanation {
        Explanation::AskedSignals => None,
        Explanation::AllSignals => measured_threshold_of(value, threshold),
    }
}

/// `--explain` のときだけ付ける、別のディレクトリと見なした段数。
fn explained_steps_of(
    separate_directory_steps: usize,
    explanation: Explanation,
) -> Option<(&'static str, Value)> {
    match explanation {
        Explanation::AskedSignals => None,
        Explanation::AllSignals => {
            Some(("separate_directory_steps", json!(separate_directory_steps)))
        }
    }
}

/// 構造類似度の値。測れていなければ、その理由。
///
/// **丸めない値を出す。** text が小数第 2 位までなのは人が読むため
/// （`similarity::Similarity` の `Display`）で、JSON の読む側は閾値との突き合わせを
/// やり直せるほうがよい。
fn structural_similarity_value_of(signal: StructuralSimilarity) -> Value {
    match signal {
        StructuralSimilarity::Measured(similarity) => {
            measured_value_of(vec![("similarity", json!(similarity.value()))])
        }
        StructuralSimilarity::NoTokens => unmeasurable_value_of("no-tokens"),
    }
}

/// 依存先の重なりの値。測れていなければ、その理由。
fn import_overlap_value_of(signal: ImportOverlap) -> Value {
    match signal {
        ImportOverlap::Measured(overlap) => {
            measured_value_of(vec![("overlap", json!(overlap.value()))])
        }
        ImportOverlap::Unavailable(cause) => imports_unavailable_value_of(cause),
    }
}

/// 依存先の集合を作れなかった理由。
fn imports_unavailable_value_of(cause: ImportsUnavailable) -> Value {
    match cause {
        ImportsUnavailable::NoDeclarations => unmeasurable_value_of("no-declarations"),
        ImportsUnavailable::ReboundSpelling => unmeasurable_value_of("rebound-spelling"),
        // **行を落とさない。** 宣言は 1 ファイルに複数あるので、行が無いと
        // 直す先も再現の手がかりも渡せない（`syntax::import::ImportsUnavailable`）
        ImportsUnavailable::UnreadableDeclaration { line } => detailed_unmeasurable_value_of(
            "unreadable-declaration",
            vec![("line", json!(line.get()))],
        ),
    }
}

/// モジュール距離の値。段数は必ず取れるので、測れなかった形にはならない。
///
/// **それでも `status` を置く。** シグナルごとに `value` の形が変わると、読む側は
/// どのキーを先に見るかをシグナルの名前から分岐することになる。
fn module_distance_value_of(distance: ModuleDistance) -> Value {
    measured_value_of(vec![("steps", json!(distance.steps()))])
}

/// 型シグネチャの単一化の可否。尋ねていないだけなら既定で `None`。
///
/// **可否は `status` ではなく値にする。** `unifiable` / `not-unifiable` を `status` に
/// 並べると、**測れたかどうか**と**重なったかどうか**が 1 つのキーに畳まれ、
/// 読む側は「測れた」を確かめるのにシグナルごとの語彙を覚えることになる。
fn type_signature_value_of(signal: TypeSignatureMatch, explanation: Explanation) -> Option<Value> {
    let value = match signal {
        TypeSignatureMatch::Unifiable => measured_value_of(vec![("unifiable", json!(true))]),
        TypeSignatureMatch::NotUnifiable => measured_value_of(vec![("unifiable", json!(false))]),
        TypeSignatureMatch::Unavailable { reason } => {
            semantics_unavailable_value_of(reason, explanation)?
        }
        TypeSignatureMatch::NoName => unmeasurable_value_of("no-name"),
        TypeSignatureMatch::NoTypeThere => unmeasurable_value_of("no-type-there"),
        TypeSignatureMatch::UnreadableHover => unmeasurable_value_of("unreadable-hover"),
        TypeSignatureMatch::ServerStillWorking => unmeasurable_value_of("server-still-working"),
        TypeSignatureMatch::UnreadableSignature => unmeasurable_value_of("unreadable-signature"),
        TypeSignatureMatch::HoverNotProvided => unmeasurable_value_of("hover-not-provided"),
        // 理由の内訳まで運ぶ。**利用者が次にすることが違う**ので、1 つのキーに畳むと
        // 直す先を選べなくなる（`rules/naming.md`「`untraced type name` と
        // `unopened` を混ぜない」）
        TypeSignatureMatch::UnopenedTypeName { reason } => detailed_unmeasurable_value_of(
            "unopened-type-name",
            vec![("unopened", Value::from(unopened_name_of(reason)))],
        ),
        TypeSignatureMatch::UntracedTypeName { reason } => detailed_unmeasurable_value_of(
            "untraced-type-name",
            vec![("untraced", Value::from(untraced_name_of(reason)))],
        ),
        TypeSignatureMatch::SiteDependentSpelling => {
            unmeasurable_value_of("site-dependent-spelling")
        }
        // 本数は測れなかった相手そのもの。**どれだけ足りないのか**を落とさない
        TypeSignatureMatch::OverloadSetMiscounted { counted, found } => {
            detailed_unmeasurable_value_of(
                "overload-set-miscounted",
                vec![("counted", json!(counted.get())), ("found", json!(found))],
            )
        }
    };

    Some(value)
}

/// 呼び出し元ドメインの重なりと分布。尋ねていないだけなら既定で `None`。
fn caller_domain_value_of(signal: &CallerDomainOverlap, explanation: Explanation) -> Option<Value> {
    let value = match signal {
        CallerDomainOverlap::Measured(measured) => measured_caller_domains_value_of(measured),
        CallerDomainOverlap::Unavailable { reason } => {
            semantics_unavailable_value_of(*reason, explanation)?
        }
        CallerDomainOverlap::NoName => unmeasurable_value_of("no-name"),
        // **印の綴りを書き写さない。** サーバごとに決まる情報なので、シグナルが
        // 運んできたものをそのまま出す（`lsp::ServerCommand::project_markers`）
        CallerDomainOverlap::ProjectUnrooted { markers } => detailed_unmeasurable_value_of(
            "project-unrooted",
            vec![("project_markers", json!(markers))],
        ),
        CallerDomainOverlap::OutsideProject { markers } => detailed_unmeasurable_value_of(
            "outside-project",
            vec![("project_markers", json!(markers))],
        ),
        CallerDomainOverlap::ProjectMembershipNotProvided => {
            unmeasurable_value_of("project-membership-not-provided")
        }
        CallerDomainOverlap::NoReferences => unmeasurable_value_of("no-references"),
        CallerDomainOverlap::UnreadableReferences => unmeasurable_value_of("unreadable-references"),
        CallerDomainOverlap::ServerStillWorking => unmeasurable_value_of("server-still-working"),
        CallerDomainOverlap::ReferencesNotProvided => {
            unmeasurable_value_of("references-not-provided")
        }
    };

    Some(value)
}

/// 測れた重なりと、両側のドメインごとの件数。
fn measured_caller_domains_value_of(measured: &MeasuredCallerDomains) -> Value {
    measured_value_of(vec![
        ("overlap", json!(measured.overlap().value())),
        ("callers_a", callers_value_of(measured.callers_a())),
        ("callers_b", callers_value_of(measured.callers_b())),
    ])
}

/// 片側のドメインごとの件数。
///
/// **ディレクトリは末尾の 1 段に縮めず、そのまま出す。** 縮めると、別の親の下にある
/// 同名のディレクトリが同じ綴りになり、分布を読み違える。
fn callers_value_of(callers: &CallerDomains) -> Value {
    let per_domain: Vec<Value> = callers
        .references_per_domain()
        .into_iter()
        .map(|(domain, references)| {
            json!({
                "domain": domain.directory().display().to_string(),
                "references": references,
            })
        })
        .collect();

    Value::Array(per_domain)
}

/// Stage 2 へ届かなかったこと。尋ねていないだけなら既定で `None`。
///
/// **尋ねていないのと測れないを `status` で分ける。** 前者はこちらが降りた話、
/// 後者は届かなかった話で、**利用者が次にすることが違う**
/// （`rules/architecture.md`「取れなかったシグナルを既定値で埋めない」）。
fn semantics_unavailable_value_of(
    reason: SemanticsUnavailable,
    explanation: Explanation,
) -> Option<Value> {
    let value = match reason {
        SemanticsUnavailable::NotAsked => match explanation {
            Explanation::AskedSignals => return None,
            Explanation::AllSignals => not_asked_value_of("stage-1-only"),
        },
        SemanticsUnavailable::NotACandidate => not_asked_value_of("not-a-candidate"),
        SemanticsUnavailable::DocumentUnopenable => unmeasurable_value_of("document-unopenable"),
        SemanticsUnavailable::WorkspaceRootUndecidable => {
            unmeasurable_value_of("workspace-root-undecidable")
        }
        SemanticsUnavailable::LspUnusable => unmeasurable_value_of("lsp-unusable"),
    };

    Some(value)
}

/// 測れたことと、その値。
///
/// `fields` はシグナルごとに違うキーと値（類似度・重なり・段数・単一化の可否）。
fn measured_value_of(fields: Vec<(&str, Value)>) -> Value {
    status_value_of(MEASURED_STATUS, fields)
}

/// 測れなかったことと、その理由。
fn unmeasurable_value_of(reason: &str) -> Value {
    detailed_unmeasurable_value_of(reason, Vec::new())
}

/// 測れなかったことと、その理由と、理由が持つ値。
///
/// `details` は理由ごとに違うキーと値（読み取れなかった行・オーバーロードの本数・
/// プロジェクトの印）。**理由のキーに畳まない**のは、text が出している値を
/// JSON で落とさないため。
fn detailed_unmeasurable_value_of(reason: &str, details: Vec<(&str, Value)>) -> Value {
    let mut fields = vec![(REASON_KEY, Value::from(reason))];
    fields.extend(details);

    status_value_of(UNMEASURABLE_STATUS, fields)
}

/// 尋ねていないことと、その理由。
fn not_asked_value_of(reason: &str) -> Value {
    status_value_of(NOT_ASKED_STATUS, vec![(REASON_KEY, Value::from(reason))])
}

/// `signal status` と、それに続く値。
///
/// **`value` を組み立てるのはここだけ。** どの `signal status` でも同じキーが先頭に来る形を
/// 1 箇所が決めていれば、読む側は `status` を見てから分岐すれば足りる
/// （`rules/naming.md` の `signal status`）。
fn status_value_of(status: &str, fields: Vec<(&str, Value)>) -> Value {
    let mut object = Map::new();
    object.insert(STATUS_KEY.to_owned(), Value::from(status));

    for (key, value) in fields {
        object.insert(key.to_owned(), value);
    }

    Value::Object(object)
}

/// シグナルが判定を傾けた向き。
fn lean_name_of(lean: Lean) -> &'static str {
    match lean {
        Lean::TowardExtract => "toward-extract",
        Lean::TowardDoNotExtract => "toward-do-not-extract",
        Lean::Neither => "neither",
    }
}

/// 比較に残る型名を開けなかった理由。
fn unopened_name_of(reason: UnopenedReason) -> &'static str {
    match reason {
        UnopenedReason::TypeDefinitionNotProvided => "type-definition-not-provided",
        UnopenedReason::NoDeclarationSite => "no-declaration-site",
        UnopenedReason::UnreadableTypeDefinition => "unreadable-type-definition",
        UnopenedReason::UnreadableDeclaringDocument => "unreadable-declaring-document",
        UnopenedReason::NoSpellingAtDeclaration => "no-spelling-at-declaration",
        UnopenedReason::UnreadableDeclarationHover => "unreadable-declaration-hover",
        UnopenedReason::HoverNotProvided => "hover-not-provided",
        UnopenedReason::ServerStillWorking => "server-still-working",
        UnopenedReason::UnopenableAlias => "unopenable-alias",
    }
}

/// 比較に残る型名を尋ねていない理由。
fn untraced_name_of(reason: UntracedReason) -> &'static str {
    match reason {
        UntracedReason::OmittedTypeAnnotation => "omitted-type-annotation",
        UntracedReason::UnalignedParameters => "unaligned-parameters",
        UntracedReason::NoTracedRecord => "no-traced-record",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::{Path, PathBuf};

    use crate::classification::signal::Signals;
    use crate::classification::{DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD, classification_of};
    use crate::semantics::caller_domain::CallerDomains;
    use crate::similarity::Similarity;
    use crate::test_support::{line, location, overload_count, scan_of_fixture};

    /// テストが渡す 0.0-1.0 の値。
    fn measured(value: f64) -> Similarity {
        Similarity::new(value).expect("テストが渡す値は 0.0-1.0")
    }

    /// 別のディレクトリにある 2 ファイルの隔たり（2 段）。
    fn separate_directories() -> ModuleDistance {
        ModuleDistance::between(
            Path::new("src/billing/discount.ts"),
            Path::new("src/inventory/reorder.ts"),
        )
    }

    /// 構造は似ていて依存先が食い違う、Stage 1 だけで組み立てたシグナル。
    fn accidental_duplication() -> Signals {
        Signals::new(
            StructuralSimilarity::Measured(measured(0.91)),
            ImportOverlap::Measured(measured(0.0)),
            separate_directories(),
        )
    }

    /// 渡したシグナルを既定の閾値で判定した JSON。
    fn json_of_signals(signals: &Signals, explanation: Explanation) -> Value {
        explained_json_of_signals(
            signals,
            DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD,
            explanation,
        )
    }

    /// 同じ組を、閾値まで指定して判定した JSON。
    fn explained_json_of_signals(
        signals: &Signals,
        threshold: Threshold,
        explanation: Explanation,
    ) -> Value {
        let text = json_of(
            &location("src/billing/discount.ts", 42),
            &location("src/inventory/reorder.ts", 18),
            &classification_of(signals, threshold),
            explanation,
        );

        serde_json::from_str(&text).expect("組み立てた綴りは JSON として読み直せる")
    }

    /// 名前で引いた根拠 1 件。
    ///
    /// # Panics
    ///
    /// その名前の根拠が出ていないとき。出ていないことを確かめるテストは
    /// [`signal_names_of`] を使う。
    fn reason_of(json: &Value, signal: &str) -> Value {
        let reasons = json["reasons"]
            .as_array()
            .expect("根拠は配列で出る")
            .iter()
            .find(|reason| reason["signal"] == signal)
            .cloned();

        let Some(reason) = reasons else {
            panic!("{signal} の根拠が出ている: {json:#}");
        };
        reason
    }

    /// 出ている根拠のシグナル名。
    fn signal_names_of(json: &Value) -> Vec<String> {
        json["reasons"]
            .as_array()
            .expect("根拠は配列で出る")
            .iter()
            .map(|reason| reason["signal"].to_string())
            .collect()
    }

    #[test]
    fn test_json_of_reports_the_verdict_with_the_same_label_as_the_text() {
        let json = json_of_signals(&accidental_duplication(), Explanation::AskedSignals);

        assert_eq!(json["verdict"], "DO-NOT-EXTRACT");
    }

    #[test]
    fn test_json_of_reports_both_locations_as_a_file_and_a_line() {
        let json = json_of_signals(&accidental_duplication(), Explanation::AskedSignals);

        assert_eq!(json["location_a"]["file"], "src/billing/discount.ts");
        assert_eq!(json["location_a"]["line"], 42);
        assert_eq!(json["location_b"]["file"], "src/inventory/reorder.ts");
        assert_eq!(json["location_b"]["line"], 18);
    }

    #[test]
    fn test_json_of_reports_the_same_suggestion_as_the_text() {
        let json = json_of_signals(&accidental_duplication(), Explanation::AskedSignals);

        assert_eq!(
            json["suggestion"],
            "偶発的な重複の可能性が高い。共通化せず分離を維持する。"
        );
    }

    #[test]
    fn test_json_of_reports_a_measured_signal_with_its_value_and_lean() {
        let json = json_of_signals(&accidental_duplication(), Explanation::AskedSignals);

        let reason = reason_of(&json, "structural-similarity");
        assert_eq!(reason["value"]["status"], "measured");
        assert_eq!(reason["value"]["similarity"], 0.91);
        assert_eq!(reason["lean"], "toward-extract");
    }

    #[test]
    fn test_json_of_without_explain_still_reports_the_structural_similarity_threshold() {
        // 既定は 0.50 なので、既定と違う閾値を渡さないと指定が効いたか分からない
        let json = explained_json_of_signals(
            &accidental_duplication(),
            Threshold::from_literal(0.75),
            Explanation::AskedSignals,
        );

        assert_eq!(reason_of(&json, "structural-similarity")["threshold"], 0.75);
    }

    #[test]
    fn test_json_of_with_an_unmeasurable_structural_similarity_omits_the_threshold() {
        // 対照は上のテスト。測れているペアでは同じキーが出る
        let signals = Signals::new(
            StructuralSimilarity::NoTokens,
            ImportOverlap::Measured(measured(0.0)),
            separate_directories(),
        );

        let json = json_of_signals(&signals, Explanation::AskedSignals);

        let reason = reason_of(&json, "structural-similarity");
        assert_eq!(reason["value"]["status"], "unmeasurable");
        assert_eq!(reason["value"]["reason"], "no-tokens");
        assert!(
            reason.get("threshold").is_none(),
            "比べていない値に閾値を並べない: {reason:#}"
        );
    }

    #[test]
    fn test_json_of_without_explain_omits_the_threshold_of_a_hardcoded_signal() {
        let json = json_of_signals(&accidental_duplication(), Explanation::AskedSignals);

        let reason = reason_of(&json, "import-overlap");
        assert_eq!(reason["value"]["overlap"], 0.0);
        assert!(
            reason.get("threshold").is_none(),
            "動かせない閾値は既定では出さない: {reason:#}"
        );
    }

    #[test]
    fn test_json_of_with_explain_reports_the_threshold_of_a_hardcoded_signal() {
        let json = json_of_signals(&accidental_duplication(), Explanation::AllSignals);

        assert!(
            reason_of(&json, "import-overlap")["threshold"].is_number(),
            "--explain は当てた閾値まで出す: {json:#}"
        );
    }

    #[test]
    fn test_json_of_with_explain_reports_the_steps_it_counted_as_separate() {
        let json = json_of_signals(&accidental_duplication(), Explanation::AllSignals);

        let reason = reason_of(&json, "module-distance");
        assert_eq!(reason["value"]["steps"], 2);
        assert!(
            reason["separate_directory_steps"].is_number(),
            "段数は閾値とは別のキーで出る: {reason:#}"
        );
        assert!(
            reason.get("threshold").is_none(),
            "0.0-1.0 の閾値と同じキーに入れない: {reason:#}"
        );
    }

    #[test]
    fn test_json_of_without_explain_omits_a_signal_it_did_not_ask_for() {
        let json = json_of_signals(&accidental_duplication(), Explanation::AskedSignals);

        let names = signal_names_of(&json);
        assert!(
            names
                .iter()
                .any(|name| name.contains("structural-similarity")),
            "尋ねたシグナルは出る: {names:?}"
        );
        assert!(
            !names
                .iter()
                .any(|name| name.contains("type-signature-match")),
            "尋ねていないシグナルは既定では出さない: {names:?}"
        );
    }

    #[test]
    fn test_json_of_with_explain_reports_a_signal_it_did_not_ask_for_as_not_asked() {
        let json = json_of_signals(&accidental_duplication(), Explanation::AllSignals);

        let reason = reason_of(&json, "type-signature-match");
        assert_eq!(reason["value"]["status"], "not-asked");
        assert_eq!(reason["value"]["reason"], "stage-1-only");
    }

    #[test]
    fn test_json_of_reports_a_unifiable_type_signature_as_a_measured_value() {
        let signals = accidental_duplication().with_semantics(
            TypeSignatureMatch::Unifiable,
            CallerDomainOverlap::NoReferences,
        );

        let json = json_of_signals(&signals, Explanation::AskedSignals);

        let reason = reason_of(&json, "type-signature-match");
        assert_eq!(reason["value"]["status"], "measured");
        assert_eq!(reason["value"]["unifiable"], true);
    }

    #[test]
    fn test_json_of_keeps_the_line_of_an_import_declaration_it_could_not_read() {
        let signals = Signals::new(
            StructuralSimilarity::Measured(measured(0.91)),
            ImportOverlap::Unavailable(ImportsUnavailable::UnreadableDeclaration {
                line: line(12),
            }),
            separate_directories(),
        );

        let json = json_of_signals(&signals, Explanation::AskedSignals);

        let value = reason_of(&json, "import-overlap")["value"].clone();
        assert_eq!(value["reason"], "unreadable-declaration");
        assert_eq!(value["line"], 12);
    }

    #[test]
    fn test_json_of_keeps_both_overload_counts_it_could_not_line_up() {
        let signals = accidental_duplication().with_semantics(
            TypeSignatureMatch::OverloadSetMiscounted {
                counted: overload_count(3),
                found: 1,
            },
            CallerDomainOverlap::NoReferences,
        );

        let json = json_of_signals(&signals, Explanation::AskedSignals);

        let value = reason_of(&json, "type-signature-match")["value"].clone();
        assert_eq!(value["reason"], "overload-set-miscounted");
        assert_eq!(value["counted"], 3);
        assert_eq!(value["found"], 1);
    }

    #[test]
    fn test_json_of_keeps_the_project_markers_the_server_looked_for() {
        let signals = accidental_duplication().with_semantics(
            TypeSignatureMatch::NoName,
            CallerDomainOverlap::ProjectUnrooted {
                markers: vec!["tsconfig.json".to_owned(), "jsconfig.json".to_owned()],
            },
        );

        let json = json_of_signals(&signals, Explanation::AskedSignals);

        let value = reason_of(&json, "caller-domain-overlap")["value"].clone();
        assert_eq!(value["reason"], "project-unrooted");
        assert_eq!(value["project_markers"][0], "tsconfig.json");
        assert_eq!(value["project_markers"][1], "jsconfig.json");
    }

    #[test]
    fn test_json_of_reports_the_caller_domains_of_both_sides() {
        let paths_a = [PathBuf::from("/repo/src/billing/invoice.ts")];
        let paths_b = [PathBuf::from("/repo/src/inventory/stock.ts")];
        let (Some(callers_a), Some(callers_b)) = (
            CallerDomains::from_reference_paths(&paths_a),
            CallerDomains::from_reference_paths(&paths_b),
        ) else {
            panic!("テストが渡す参照元は 1 件以上");
        };
        let signals = accidental_duplication().with_semantics(
            TypeSignatureMatch::NoName,
            CallerDomainOverlap::Measured(MeasuredCallerDomains::new(callers_a, callers_b)),
        );

        let json = json_of_signals(&signals, Explanation::AskedSignals);

        let value = reason_of(&json, "caller-domain-overlap")["value"].clone();
        assert_eq!(value["overlap"], 0.0);
        assert_eq!(value["callers_a"][0]["domain"], "/repo/src/billing");
        assert_eq!(value["callers_a"][0]["references"], 1);
        assert_eq!(value["callers_b"][0]["domain"], "/repo/src/inventory");
    }

    /// `tests/fixtures/` 配下のディレクトリを走査した JSON。
    fn scan_json_of_fixture(relative_path: &str) -> Value {
        let scan = scan_of_fixture(relative_path, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);
        let text = scan_json_of(&scan, Explanation::AskedSignals);

        serde_json::from_str(&text).expect("組み立てた綴りは JSON として読み直せる")
    }

    #[test]
    fn test_scan_json_of_lists_a_candidate_pair_in_the_same_shape_as_compare() {
        let json = scan_json_of_fixture("scan");

        let pair = json["candidate_pairs"][0].clone();
        assert_eq!(pair["verdict"], "DO-NOT-EXTRACT");
        assert!(
            pair["location_a"]["file"]
                .as_str()
                .unwrap_or_default()
                .ends_with("billing/discount.ts"),
            "候補ペアの位置が compare と同じ形で出る: {pair:#}"
        );
        assert_eq!(pair["location_a"]["line"], 3);
    }

    #[test]
    fn test_scan_json_of_reports_how_much_of_the_codebase_it_walked() {
        let json = scan_json_of_fixture("scan");

        let walked = json["walked"].clone();
        assert_eq!(walked["files"], 8);
        assert_eq!(walked["chunks"], 6);
        assert_eq!(walked["compared_pairs"], 14);
        assert_eq!(walked["pruned_pairs"], 5);
    }

    #[test]
    fn test_scan_json_of_reports_a_file_it_could_not_read_with_its_cause() {
        let json = scan_json_of_fixture("scan-skipped");

        let skipped = json["skipped_files"][0].clone();
        assert!(
            skipped["file"]
                .as_str()
                .unwrap_or_default()
                .ends_with("not-utf8.ts"),
            "どのファイルを飛ばしたかが出る: {skipped:#}"
        );
        assert_eq!(skipped["reason"], "source-unreadable");
        assert!(
            skipped["cause"].is_string(),
            "読めなかった理由まで出る: {skipped:#}"
        );
    }

    #[test]
    fn test_scan_json_of_reports_a_function_it_could_not_chunk() {
        let json = scan_json_of_fixture("scan-skipped");

        let unchunkable = json["unchunkable"][0].clone();
        assert!(
            unchunkable["file"]
                .as_str()
                .unwrap_or_default()
                .ends_with("unterminated.ts"),
            "どの関数を飛ばしたかが出る: {unchunkable:#}"
        );
        assert_eq!(unchunkable["line"], 1);
    }

    #[test]
    fn test_scan_json_of_without_anything_skipped_reports_empty_arrays() {
        // 対照は上の 2 つ。飛ばしたものがある走査では同じキーに要素が並ぶ
        let json = scan_json_of_fixture("scan");

        assert_eq!(json["skipped_files"].as_array().map(Vec::len), Some(0));
        assert_eq!(json["unchunkable"].as_array().map(Vec::len), Some(0));
    }
}
