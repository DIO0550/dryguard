//! references の応答から、参照元のファイルと位置を取り出す。
//!
//! **サーバとの往復そのものは `connection` が持つ。** ここにあるのは受け取った応答を
//! こちらが読める形へ直す変換だけなので、サーバを起動せずに確かめられる
//! (rules/tdd.md「`lsp` は『応答を受け取ってから先』を切り出す」)。

use std::path::{Path, PathBuf};

use crate::source_position::SourcePosition;

use super::uri::{self, UriPathError};

/// 名前を使っているファイルと、その参照の開始位置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    path: PathBuf,
    position: SourcePosition,
}

impl Reference {
    /// 参照元のファイル。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 参照の開始位置。列は UTF-16 のコード単位。
    pub fn position(&self) -> SourcePosition {
        self.position
    }

    /// テストが用意する、サーバ応答相当の参照。
    #[cfg(test)]
    pub(crate) fn new(path: PathBuf, position: SourcePosition) -> Self {
        Self { path, position }
    }
}

/// references に尋ねた結果。
///
/// **「取れなかった」を 1 つにまとめない。** どれなのかで**利用者が次に試すことが違う**
/// （サーバを替える / そのコードベースの見せ方を直す / dryguard 側の穴）
/// (`rules/architecture.md`「取れなかったシグナルを既定値で埋めない」)。
#[derive(Debug)]
pub enum ReferencesOutcome {
    /// 参照元のファイルと位置が返った。
    ///
    /// **同じファイルに 2 件あれば 2 つ並ぶ。** ドメインごとの件数
    /// （`billing 3件 / inventory 5件`）が判定の根拠になるので、ファイル単位で畳まない。
    Answered(Vec<Reference>),
    /// サーバはこの位置の参照元を返さなかった。
    ///
    /// **空配列もここに入れる。** tsserver は「呼び出し元が本当に無い」ときと
    /// 「そのファイルをプロジェクトとして見ていない」ときの**両方で空配列を返す**ので、
    /// 材料が取れたことにすると、後段は 0 件を「別ドメインに散っていない」と読む。
    NoAnswer,
    /// 参照元は返ったが、パスとして読めない URI が混じっていた。
    ///
    /// **読めた分だけを返さない。** ドメインごとの件数が黙って目減りし、
    /// 「呼び出し元が少ない」のか「読めていない」のかを後段が区別できなくなる。
    Unreadable {
        /// 読めなかった理由。どの URI で落ちたかを持つ。
        cause: UriPathError,
    },
    /// 尋ねるたびにサーバが作業を始めるので、落ち着いた答えを受け取れなかった。
    ///
    /// **途中の答えを最終的なシグナルとして返さない。** 作業中の答えは
    /// まだ見ていないファイルの分が抜けており、呼び出し元の分布が実際より狭く出る
    /// （`rules/architecture.md`「取れなかったシグナルを既定値で埋めない」）。
    ServerStillWorking,
    /// サーバが references を提供していない。要求は送っていない。
    NotSupported,
}

/// references の応答を、読み取れたかどうかが分かる形にする。
///
/// `locations` はサーバが返した参照元。返す順はサーバが返した順のまま。
///
/// ドメインの導出（どのディレクトリに属するか）は `semantics` が行う。ここは応答の形を
/// 読むところまで。
pub(super) fn outcome_of(locations: &[lsp_types::Location]) -> ReferencesOutcome {
    if locations.is_empty() {
        return ReferencesOutcome::NoAnswer;
    }

    let mut paths = Vec::with_capacity(locations.len());

    for location in locations {
        match uri::path_of(&location.uri) {
            Ok(path) => paths.push(Reference {
                path,
                position: SourcePosition::from_lsp_position(location.range.start),
            }),
            Err(cause) => return ReferencesOutcome::Unreadable { cause },
        }
    }

    ReferencesOutcome::Answered(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::str::FromStr;

    use lsp_types::{Location, Position, Range, Uri};

    #[test]
    fn test_references_outcome_preserves_the_utf16_position_of_each_reference() {
        let mut location = reference("file:///repo/src/lib.rs");
        location.range = Range::new(Position::new(3, 17), Position::new(3, 23));
        let ReferencesOutcome::Answered(references) = outcome_of(&[location]) else {
            panic!("参照元が返る");
        };
        assert_eq!(
            references[0].position().to_lsp_position(),
            Position::new(3, 17)
        );
    }

    /// サーバが返す形の参照元 1 件。
    ///
    /// このヘルパーでは行頭を指し、位置を検証するテストは範囲を上書きする。
    fn reference(uri: &str) -> Location {
        Location {
            uri: Uri::from_str(uri).expect("テストが渡す文字列は URI として読める"),
            range: Range::new(Position::new(0, 0), Position::new(0, 0)),
        }
    }

    /// 読み取れた参照元のパス。読み取れていなければテストを落とす。
    fn paths_of(locations: &[Location]) -> Vec<PathBuf> {
        match outcome_of(locations) {
            ReferencesOutcome::Answered(paths) => paths
                .iter()
                .map(|reference| reference.path().to_path_buf())
                .collect(),
            other => panic!("参照元を読み取れる: {other:?}"),
        }
    }

    #[test]
    fn test_references_outcome_of_answers_in_separate_files_keeps_every_file() {
        let locations = [
            reference("file:///repo/src/billing/invoice.ts"),
            reference("file:///repo/src/inventory/stock.ts"),
        ];

        assert_eq!(
            paths_of(&locations),
            vec![
                PathBuf::from("/repo/src/billing/invoice.ts"),
                PathBuf::from("/repo/src/inventory/stock.ts"),
            ]
        );
    }

    #[test]
    fn test_references_outcome_of_two_answers_in_one_file_keeps_both() {
        // 対照は上のテスト。同じファイルから 2 件で、畳むと 1 件になる。
        // ドメインごとの件数が判定の根拠なので、ここで畳むと数が出せない
        let locations = [
            reference("file:///repo/src/billing/invoice.ts"),
            reference("file:///repo/src/billing/invoice.ts"),
        ];

        assert_eq!(paths_of(&locations).len(), 2);
    }

    #[test]
    fn test_references_outcome_of_an_encoded_uri_is_the_path_it_spells() {
        let locations = [reference("file:///repo/my%20project/invoice.ts")];

        assert_eq!(
            paths_of(&locations),
            vec![Path::new("/repo/my project/invoice.ts")]
        );
    }

    #[test]
    fn test_references_outcome_of_no_answers_is_not_an_empty_answer() {
        // 空配列は「呼び出し元が無い」と「そのファイルをプロジェクトとして見ていない」の
        // 両方で返る。Answered(vec![]) にすると、後段が 0 件を材料として読む
        assert!(matches!(outcome_of(&[]), ReferencesOutcome::NoAnswer));
    }

    #[test]
    fn test_references_outcome_of_answers_including_an_unreadable_uri_is_not_partially_read() {
        // 対照として読める URI を 1 件置く。読めた分だけ返すと、この入力は
        // 「参照元 1 件」に見え、読めなかったことが数から消える
        let locations = [
            reference("file:///repo/src/billing/invoice.ts"),
            reference("untitled:Untitled-1"),
        ];

        assert!(matches!(
            outcome_of(&locations),
            ReferencesOutcome::Unreadable { .. }
        ));
    }
}
