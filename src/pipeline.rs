//! ステージを呼ぶ順序。
//!
//! ここが持つのは順序だけで、読み込みは `Location`、切り出しは `Chunk` にある。
//! `syntax` は I/O を持たないので、読んだ結果を渡す形になる
//! （rules/coding.md 禁止事項 / rules/architecture.md「3 ステージのパイプライン」）。

use std::error::Error;
use std::fmt;
use std::io;

use crate::classification::signal::{ImportOverlap, Signals, StructuralSimilarity};
use crate::location::Location;
use crate::syntax::chunk::{Chunk, ChunkingError};
use crate::syntax::module_distance::ModuleDistance;
use crate::syntax::token::TokenSet;

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
fn chunk_at(location: &Location) -> Result<Chunk, ChunkPairError> {
    let source = location
        .read_source()
        .map_err(|cause| ChunkPairError::SourceUnreadable {
            location: location.clone(),
            cause,
        })?;

    Chunk::find_enclosing(location, &source).map_err(|cause| ChunkPairError::ChunkingFailed {
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

/// 正規化トークン集合の Jaccard 係数。どちらかにトークンが無ければ測れない。
fn structural_similarity_of(chunk_a: &Chunk, chunk_b: &Chunk) -> StructuralSimilarity {
    let (Some(tokens_a), Some(tokens_b)) = (
        TokenSet::from_source(chunk_a.source()),
        TokenSet::from_source(chunk_b.source()),
    ) else {
        return StructuralSimilarity::NoTokens;
    };

    StructuralSimilarity::Measured(tokens_a.jaccard(&tokens_b))
}

/// 依存先集合の Jaccard 係数。どちらかのファイルに import が無ければ測れない。
fn import_overlap_of(chunk_a: &Chunk, chunk_b: &Chunk) -> ImportOverlap {
    let (Some(imports_a), Some(imports_b)) = (chunk_a.imports(), chunk_b.imports()) else {
        return ImportOverlap::NoImports;
    };

    ImportOverlap::Measured(imports_a.jaccard(imports_b))
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
    /// ファイルは読めたが、チャンクを切り出せなかった。
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
            Self::ChunkingFailed { cause, .. } => Some(cause),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::similarity::Similarity;
    use crate::test_support::line;

    fn measured(value: f64) -> Similarity {
        Similarity::new(value).expect("テストが渡す値は 0.0-1.0")
    }

    /// ファイルを読まずにチャンクを作る。
    ///
    /// `Chunk::find_enclosing` はソースを引数で受けるので、実ファイルが要るのは
    /// 読み込みまで。ここで見たいのは読み込みの後ろにあるシグナルの測り方なので、
    /// パスは位置の材料としてだけ渡す。
    fn chunk_of(path: &str, number: usize, source: &str) -> Chunk {
        let location = Location::new(PathBuf::from(path), line(number));

        Chunk::find_enclosing(&location, source).expect("テストが渡す位置は関数の中を指している")
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
}
