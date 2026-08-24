//! ステージを呼ぶ順序。
//!
//! ここが持つのは順序だけで、読み込みは `Location`、切り出しは `Chunk` にある。
//! `syntax` は I/O を持たないので、読んだ結果を渡す形になる
//! （rules/coding.md 禁止事項 / rules/architecture.md「3 ステージのパイプライン」）。

use std::error::Error;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use crate::classification::signal::{ImportOverlap, Signals, StructuralSimilarity};
use crate::classification::{Classification, classification_of, is_structurally_similar};
use crate::codebase::{CodebaseError, source_of, typescript_paths_of};
use crate::location::Location;
use crate::syntax::chunk::{Chunk, ChunkingError, chunks_of};
use crate::syntax::module_distance::ModuleDistance;
use crate::syntax::tree::{ParseError, SyntaxTree};
use crate::threshold::Threshold;

/// 比較する 2 箇所から、チャンクの組を取り出す。
///
/// # Errors
///
/// どちらかのファイルが読めない / どちらかのチャンクを切り出せないとき。
/// どちらの位置で失敗したかはエラーが持つ。
pub fn chunk_pair_of(
    location_a: &Location,
    location_b: &Location,
) -> Result<(Chunk, Chunk), ChunkPairError> {
    Ok((chunk_at(location_a)?, chunk_at(location_b)?))
}

/// その位置のファイルを読んで、指定行を含む関数を切り出す。
///
/// パースはファイルにつき 1 回にする。チャンクの範囲も import の集合も同じ木から
/// 採るので、ソースを渡す形のままだと 1 箇所につき 2 回パースすることになる。
fn chunk_at(location: &Location) -> Result<Chunk, ChunkPairError> {
    let source = location
        .read_source()
        .map_err(|cause| ChunkPairError::SourceUnreadable {
            location: location.clone(),
            cause,
        })?;

    let tree =
        SyntaxTree::from_typescript(&source).map_err(|cause| ChunkPairError::SourceUnparsable {
            location: location.clone(),
            cause,
        })?;

    Chunk::find_enclosing(location, &tree).map_err(|cause| ChunkPairError::ChunkingFailed {
        location: location.clone(),
        cause,
    })
}

/// チャンクの組から、判定の材料になるシグナルを測る。
///
/// 測れなかったシグナルは、その理由を持つバリアントで返る（値は埋めない）。
/// **測るのはここ、判定は `classification`** という分担にする
/// (`rules/architecture.md`「3 ステージのパイプライン」)。
pub fn signals_of(chunk_a: &Chunk, chunk_b: &Chunk) -> Signals {
    Signals::new(
        structural_similarity_of(chunk_a, chunk_b),
        import_overlap_of(chunk_a, chunk_b),
        ModuleDistance::between(chunk_a.path(), chunk_b.path()),
    )
}

/// 正規化トークン列の似かた。どちらかにトークンが無ければ測れない。
fn structural_similarity_of(chunk_a: &Chunk, chunk_b: &Chunk) -> StructuralSimilarity {
    let (Some(tokens_a), Some(tokens_b)) = (chunk_a.tokens(), chunk_b.tokens()) else {
        return StructuralSimilarity::NoTokens;
    };

    StructuralSimilarity::Measured(tokens_a.similarity_with(tokens_b))
}

/// 依存先集合の Jaccard 係数。どちらかのファイルに import が無ければ測れない。
fn import_overlap_of(chunk_a: &Chunk, chunk_b: &Chunk) -> ImportOverlap {
    let (Some(imports_a), Some(imports_b)) = (chunk_a.imports(), chunk_b.imports()) else {
        return ImportOverlap::NoImports;
    };

    ImportOverlap::Measured(imports_a.jaccard(imports_b))
}

/// コードベース全体を走査して、候補ペアを判定する。
///
/// `root` は走査を始めるディレクトリ、`structural_similarity_threshold` は
/// 候補ペアとして拾う構造類似度の下限。
///
/// 読めなかったファイルと切り出せなかった関数は、走査を止めずに結果へ残す。
/// **1 ファイルのために全体を落とすと、他のペアの判定まで失われる**。
///
/// # Errors
///
/// `root` がディレクトリでない / 途中のディレクトリを読めないとき。
pub fn scan_of(
    root: &Path,
    structural_similarity_threshold: Threshold,
) -> Result<Scan, CodebaseError> {
    let paths = typescript_paths_of(root)?;
    let file_count = paths.len();

    let mut chunks: Vec<Chunk> = Vec::new();
    let mut skipped_files = Vec::new();
    let mut unchunkable = Vec::new();

    for path in paths {
        let source = match source_of(&path) {
            Ok(source) => source,
            Err(cause) => {
                skipped_files.push(SkippedFile::SourceUnreadable { path, cause });
                continue;
            }
        };

        let tree = match SyntaxTree::from_typescript(&source) {
            Ok(tree) => tree,
            Err(cause) => {
                skipped_files.push(SkippedFile::SourceUnparsable { path, cause });
                continue;
            }
        };

        let file_chunks = chunks_of(&path, &tree);
        unchunkable.extend(
            file_chunks
                .unparsable_starts()
                .iter()
                .map(|start| Location::new(path.clone(), *start)),
        );
        chunks.extend(file_chunks.chunks().iter().cloned());
    }

    Ok(scan_of_chunks(
        &chunks,
        structural_similarity_threshold,
        ScanInputs {
            file_count,
            skipped_files,
            unchunkable,
        },
    ))
}

/// 走査で集めた、チャンク以外の材料。
///
/// [`scan_of_chunks`] の引数をまとめるためだけの型なので公開しない。
struct ScanInputs {
    file_count: usize,
    skipped_files: Vec<SkippedFile>,
    unchunkable: Vec<Location>,
}

/// 集めたチャンクを総当たりで比べ、候補ペアだけを判定する。
///
/// 候補かどうかは [`is_structurally_similar`] に聞く。**同じ条件をここに書き直すと、
/// 判定が「似ている」と見なす範囲と候補に拾う範囲が黙ってずれる**
/// (`rules/architecture.md`「判定は 1 箇所にだけ置く」)。
fn scan_of_chunks(
    chunks: &[Chunk],
    structural_similarity_threshold: Threshold,
    inputs: ScanInputs,
) -> Scan {
    let mut candidate_pairs = Vec::new();
    let mut compared_pair_count = 0;

    for (index, chunk_a) in chunks.iter().enumerate() {
        for chunk_b in &chunks[index + 1..] {
            if is_nested(chunk_a, chunk_b) {
                continue;
            }
            compared_pair_count += 1;

            let signals = signals_of(chunk_a, chunk_b);
            if !is_structurally_similar(
                signals.structural_similarity(),
                structural_similarity_threshold,
            ) {
                continue;
            }

            candidate_pairs.push(CandidatePair {
                location_a: start_of(chunk_a),
                location_b: start_of(chunk_b),
                classification: classification_of(&signals, structural_similarity_threshold),
            });
        }
    }

    Scan {
        candidate_pairs,
        file_count: inputs.file_count,
        chunk_count: chunks.len(),
        compared_pair_count,
        skipped_files: inputs.skipped_files,
        unchunkable: inputs.unchunkable,
    }
}

/// 同じファイルにあって範囲が重なる 2 つのチャンクか（外側の関数とその中の関数）。
///
/// 入れ子の組は同じコードを二重に数えているだけなので比べない。**別のファイルに
/// 同じ形があれば見つけたい**ので、内側のチャンク自体は落とさない。
fn is_nested(chunk_a: &Chunk, chunk_b: &Chunk) -> bool {
    let same_file = chunk_a.path() == chunk_b.path();

    same_file && chunk_a.lines().overlaps(chunk_b.lines())
}

/// そのチャンクの始まりを指す位置。`compare` にそのまま渡せる形。
fn start_of(chunk: &Chunk) -> Location {
    Location::new(chunk.path().to_path_buf(), chunk.lines().start())
}

/// コードベース全体を走査した結果。
///
/// 候補ペアだけでなく、走査した量と飛ばしたものも持つ。**出た候補の数だけでは
/// 「見ていないもの」が分からない**（`rules/architecture.md`
/// 「取れなかったシグナルを既定値で埋めない」）。
#[derive(Debug)]
pub struct Scan {
    candidate_pairs: Vec<CandidatePair>,
    file_count: usize,
    chunk_count: usize,
    compared_pair_count: usize,
    skipped_files: Vec<SkippedFile>,
    unchunkable: Vec<Location>,
}

impl Scan {
    /// 構造類似度が閾値に届いたペアと、その判定。列挙した順に並ぶ。
    pub fn candidate_pairs(&self) -> &[CandidatePair] {
        &self.candidate_pairs
    }

    /// 走査の対象になった TypeScript ファイルの数。
    pub fn file_count(&self) -> usize {
        self.file_count
    }

    /// 切り出せたチャンクの数。
    pub fn chunk_count(&self) -> usize {
        self.chunk_count
    }

    /// 実際に比べたペアの数。入れ子の組は含まない。
    pub fn compared_pair_count(&self) -> usize {
        self.compared_pair_count
    }

    /// 読めなかった・構文木にできなかったファイル。
    pub fn skipped_files(&self) -> &[SkippedFile] {
        &self.skipped_files
    }

    /// 構文エラーで切り出せなかった関数の位置。
    pub fn unchunkable(&self) -> &[Location] {
        &self.unchunkable
    }
}

/// 構造類似度が閾値に届いたペアと、その判定。
#[derive(Debug)]
pub struct CandidatePair {
    location_a: Location,
    location_b: Location,
    classification: Classification,
}

impl CandidatePair {
    /// 先に列挙したほうのチャンクの位置。
    pub fn location_a(&self) -> &Location {
        &self.location_a
    }

    /// 後に列挙したほうのチャンクの位置。
    pub fn location_b(&self) -> &Location {
        &self.location_b
    }

    /// このペアの判定と根拠。
    pub fn classification(&self) -> &Classification {
        &self.classification
    }
}

/// 走査の途中で飛ばしたファイルと、その理由。
///
/// どのファイルを飛ばしたかを残すのは、黙って落とすと**対象に入っていたのか
/// 除外されたのか**を読む側が区別できないため。
#[derive(Debug)]
pub enum SkippedFile {
    /// ファイルを読めなかった。
    SourceUnreadable { path: PathBuf, cause: io::Error },
    /// ファイルは読めたが、構文木にできなかった。
    SourceUnparsable { path: PathBuf, cause: ParseError },
}

impl fmt::Display for SkippedFile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SourceUnreadable { path, cause } => {
                write!(formatter, "{}: 読めません: {cause}", path.display())
            }
            Self::SourceUnparsable { path, cause } => {
                write!(formatter, "{}: 読み解けません: {cause}", path.display())
            }
        }
    }
}

impl Error for SkippedFile {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::SourceUnreadable { cause, .. } => Some(cause),
            Self::SourceUnparsable { cause, .. } => Some(cause),
        }
    }
}

/// チャンクを取り出せなかった理由。
///
/// どちらの位置で失敗したかを持つ。`compare` は 2 箇所を受け取るので、
/// 位置が分からないと利用者はどちらを直せばよいか分からない。
#[derive(Debug)]
pub enum ChunkPairError {
    /// ファイルを読めなかった。
    SourceUnreadable {
        location: Location,
        cause: io::Error,
    },
    /// ファイルは読めたが、構文木にできなかった。
    SourceUnparsable {
        location: Location,
        cause: ParseError,
    },
    /// 構文木にはできたが、チャンクを切り出せなかった。
    ChunkingFailed {
        location: Location,
        cause: ChunkingError,
    },
}

impl fmt::Display for ChunkPairError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SourceUnreadable { location, cause } => {
                write!(formatter, "{location} のファイルを読めません: {cause}")
            }
            Self::SourceUnparsable { location, cause } => {
                write!(formatter, "{location} のファイルを読み解けません: {cause}")
            }
            Self::ChunkingFailed { location, cause } => {
                write!(formatter, "{location} から切り出せません: {cause}")
            }
        }
    }
}

impl Error for ChunkPairError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::SourceUnreadable { cause, .. } => Some(cause),
            Self::SourceUnparsable { cause, .. } => Some(cause),
            Self::ChunkingFailed { cause, .. } => Some(cause),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::classification::DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD;
    use crate::classification::verdict::Verdict;
    use crate::similarity::Similarity;
    use crate::test_support::line;

    fn measured(value: f64) -> Similarity {
        Similarity::new(value).expect("テストが渡す値は 0.0-1.0")
    }

    /// ファイルを読まずにチャンクを作る。
    ///
    /// `Chunk::find_enclosing` は構文木を引数で受けるので、実ファイルが要るのは
    /// 読み込みまで。ここで見たいのは読み込みの後ろにあるシグナルの測り方なので、
    /// パスは位置の材料としてだけ渡す。
    fn chunk_of(path: &str, number: usize, source: &str) -> Chunk {
        let location = Location::new(PathBuf::from(path), line(number));
        let tree = SyntaxTree::from_typescript(source).expect("テストが渡すソースは木にできる");

        Chunk::find_enclosing(&location, &tree).expect("テストが渡す位置は関数の中を指している")
    }

    const WITH_IMPORT: &str = "import { pad } from \"./pad\";\n\
                               export function format(value: number): string {\n\
                               \x20 return pad(value);\n\
                               }\n";

    const WITHOUT_IMPORT: &str = "export function format(value: number): string {\n\
                                  \x20 return String(value);\n\
                                  }\n";

    #[test]
    fn test_signals_of_two_chunks_depending_on_the_same_module_measure_a_total_overlap() {
        let chunk_a = chunk_of("src/utils/format.ts", 2, WITH_IMPORT);
        let chunk_b = chunk_of("src/utils/render.ts", 2, WITH_IMPORT);

        let signals = signals_of(&chunk_a, &chunk_b);

        assert_eq!(
            signals.import_overlap(),
            ImportOverlap::Measured(measured(1.0))
        );
    }

    #[test]
    fn test_signals_of_a_chunk_whose_file_has_no_import_cannot_measure_the_overlap() {
        // 対照は上のテスト。同じ測り方で片方だけ import を外している
        let chunk_a = chunk_of("src/utils/format.ts", 2, WITH_IMPORT);
        let chunk_b = chunk_of("src/utils/render.ts", 1, WITHOUT_IMPORT);

        let signals = signals_of(&chunk_a, &chunk_b);

        assert_eq!(signals.import_overlap(), ImportOverlap::NoImports);
    }

    #[test]
    fn test_signals_of_two_identical_chunks_measure_a_total_structural_similarity() {
        let chunk_a = chunk_of("src/utils/format.ts", 2, WITH_IMPORT);
        let chunk_b = chunk_of("src/report/format.ts", 2, WITH_IMPORT);

        let signals = signals_of(&chunk_a, &chunk_b);

        assert_eq!(
            signals.structural_similarity(),
            StructuralSimilarity::Measured(measured(1.0))
        );
    }

    #[test]
    fn test_signals_of_two_chunks_in_sibling_directories_measure_both_steps() {
        let chunk_a = chunk_of("src/utils/format.ts", 2, WITH_IMPORT);
        let chunk_b = chunk_of("src/report/format.ts", 2, WITH_IMPORT);

        let signals = signals_of(&chunk_a, &chunk_b);

        assert_eq!(signals.module_distance().steps(), 2);
    }

    /// `tests/fixtures/` 配下のディレクトリ。
    ///
    /// カレントディレクトリではなくマニフェストの位置から組み立てる
    /// （テストの実行位置に依存させない）。
    fn fixture(relative_path: &str) -> PathBuf {
        PathBuf::from(format!(
            "{}/tests/fixtures/{relative_path}",
            env!("CARGO_MANIFEST_DIR")
        ))
    }

    /// 既定の閾値で走査した結果。
    fn scan_of_fixture(relative_path: &str) -> Scan {
        scan_of(
            &fixture(relative_path),
            DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD,
        )
        .expect("フィクスチャのディレクトリは走査できる")
    }

    #[test]
    fn test_scan_of_a_codebase_reports_a_similar_pair_in_separate_domains() {
        let scan = scan_of_fixture("scan");

        let pairs: Vec<String> = scan
            .candidate_pairs()
            .iter()
            .map(|pair| format!("{} <-> {}", pair.location_a(), pair.location_b()))
            .collect();
        assert!(
            pairs
                .iter()
                .any(|pair| pair.contains("billing/discount.ts:3")
                    && pair.contains("inventory/reorder.ts:3")),
            "構造の同じ 2 関数が候補ペアとして出る: {pairs:?}"
        );
    }

    #[test]
    fn test_scan_of_a_codebase_classifies_a_candidate_pair_without_shared_dependencies_as_do_not_extract()
     {
        let scan = scan_of_fixture("scan");

        let verdicts: Vec<Verdict> = scan
            .candidate_pairs()
            .iter()
            .map(|pair| pair.classification().verdict())
            .collect();
        assert!(
            verdicts.contains(&Verdict::DoNotExtract),
            "依存先を共有しない候補ペアに判定が付く: {verdicts:?}"
        );
    }

    #[test]
    fn test_scan_of_a_codebase_does_not_compare_a_function_with_the_one_nested_in_it() {
        // フィクスチャのチャンクは 4 つ（discount / reorder / makeAdder とその中のアロー）。
        // 総当たりなら 6 ペアだが、入れ子の 1 ペアは比較しないので 5 ペアになる
        let scan = scan_of_fixture("scan");

        assert_eq!(scan.chunk_count(), 4, "切り出せたチャンクの数");
        assert_eq!(
            scan.compared_pair_count(),
            5,
            "入れ子の組（makeAdder とその中のアロー）は比べない"
        );
    }

    #[test]
    fn test_scan_of_a_codebase_counts_the_files_it_walked() {
        let scan = scan_of_fixture("scan");

        assert_eq!(
            scan.file_count(),
            5,
            "除外したディレクトリと TypeScript でないファイルは数えない"
        );
    }

    #[test]
    fn test_scan_of_a_codebase_with_an_unreadable_file_keeps_the_path_and_the_reason() {
        // 対照として同じディレクトリに読めるファイルを 1 つ置いてある。
        // 読めない 1 件で走査全体が止まらないことも、ここで見る
        let scan = scan_of_fixture("scan-skipped");

        let skipped: Vec<String> = scan
            .skipped_files()
            .iter()
            .map(SkippedFile::to_string)
            .collect();
        assert_eq!(skipped.len(), 1, "飛ばしたのは 1 件: {skipped:?}");
        assert!(
            skipped[0].contains("not-utf8.ts"),
            "どのファイルを飛ばしたかが残る: {skipped:?}"
        );
        assert_eq!(
            scan.chunk_count(),
            1,
            "読めたファイルの関数は切り出せている"
        );
    }

    #[test]
    fn test_scan_of_a_codebase_with_a_broken_function_keeps_where_it_could_not_chunk() {
        // 対照は同じディレクトリの sound.ts。壊れた関数だけがチャンクから外れる
        let scan = scan_of_fixture("scan-skipped");

        let unchunkable: Vec<String> = scan.unchunkable().iter().map(Location::to_string).collect();
        assert_eq!(
            unchunkable.len(),
            1,
            "切り出せなかったのは 1 件: {unchunkable:?}"
        );
        assert!(
            unchunkable[0].ends_with("unterminated.ts:1"),
            "どの関数を飛ばしたかが位置で残る: {unchunkable:?}"
        );
    }

    #[test]
    fn test_scan_of_a_missing_directory_reports_the_root_it_was_given() {
        let root = fixture("scan/missing");

        let result = scan_of(&root, DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD);

        let Err(CodebaseError::RootNotADirectory { root: reported }) = result else {
            panic!("ディレクトリでない根は RootNotADirectory になる");
        };
        assert_eq!(reported, root);
    }
}
