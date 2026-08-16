//! ステージを呼ぶ順序。
//!
//! ファイルの読み込みはここが持つ。`syntax` は I/O を持たないので、その外側で
//! 読んで渡す（rules/coding.md 禁止事項 / rules/architecture.md「3 ステージのパイプライン」）。

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;

use crate::location::Location;
use crate::syntax::chunk::{Chunk, ChunkingError};

/// 比較する 2 箇所から、それぞれのチャンクを取り出す。
///
/// # Errors
///
/// どちらかのファイルが読めない / どちらかのチャンクを切り出せないとき。
/// どちらの位置で失敗したかはエラーが持つ。
pub fn collect_chunks(
    location_a: &Location,
    location_b: &Location,
) -> Result<(Chunk, Chunk), ChunkCollectionError> {
    Ok((chunk_at(location_a)?, chunk_at(location_b)?))
}

/// その位置のファイルを読んで、指定行を含む関数を切り出す。
fn chunk_at(location: &Location) -> Result<Chunk, ChunkCollectionError> {
    let source = fs::read_to_string(location.path()).map_err(|cause| {
        ChunkCollectionError::SourceUnreadable {
            location: location.clone(),
            cause,
        }
    })?;

    Chunk::find_enclosing(location, &source).map_err(|cause| ChunkCollectionError::ChunkingFailed {
        location: location.clone(),
        cause,
    })
}

/// チャンクを取り出せなかった理由。
///
/// どちらの位置で失敗したかを持つ。`compare` は 2 箇所を受け取るので、
/// 位置が分からないと利用者はどちらを直せばよいか分からない。
#[derive(Debug)]
pub enum ChunkCollectionError {
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

impl fmt::Display for ChunkCollectionError {
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

impl Error for ChunkCollectionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::SourceUnreadable { cause, .. } => Some(cause),
            Self::ChunkingFailed { cause, .. } => Some(cause),
        }
    }
}
