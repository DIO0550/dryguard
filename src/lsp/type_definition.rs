//! typeDefinition の応答から、型が宣言されている場所を取り出す。
//!
//! **サーバとの往復そのものは `connection` が持つ。** ここにあるのは受け取った応答を
//! こちらが読める形へ直す変換だけなので、サーバを起動せずに確かめられる
//! (rules/tdd.md「`lsp` は『応答を受け取ってから先』を切り出す」)。

use std::path::{Path, PathBuf};

use lsp_types::{GotoDefinitionResponse, Location, LocationLink, Uri};

use super::uri::{self, PathUriError, UriPathError};
use crate::source_position::SourcePosition;

/// 型が宣言されている場所。ファイルと、その中の 1 点。
///
/// **位置まで持つ。** 型エイリアスの右辺は、宣言の位置へ hover を送らないと返らない
/// （使用側の位置では `import Amount` としか返らない）。
///
/// パスと URI の両方を持つのは、**開かせるのと尋ね直すのとで要る形が違う**ため。
/// どちらも生成時に同じ応答から作るので、食い違った組は作れない
/// (rules/coding.md「不正な状態を型で表現できなくする」)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclarationSite {
    uri: Uri,
    path: PathBuf,
    position: SourcePosition,
}

impl DeclarationSite {
    /// 宣言があるファイルと、その中の 1 点から作る。
    ///
    /// **URI はパスから導く。** 2 つを別々に受け取ると、互いに食い違った組を作れてしまう
    /// (`rules/coding.md`「生成時に検証し、不正な値を存在させない」)。応答から作る側
    /// （[`outcome_of`]）はサーバが返した URI の綴りをそのまま持つので、ここは通らない。
    ///
    /// # Errors
    ///
    /// `path` を `file:` URI にできないとき（絶対パスでない・`.` / `..` が残っている・
    /// `file:` URI で表せない前置きを持つ）。
    pub fn new(path: &Path, position: SourcePosition) -> Result<Self, PathUriError> {
        Ok(Self {
            uri: uri::file_uri_of(path)?,
            path: path.to_path_buf(),
            position,
        })
    }

    /// 宣言があるファイル。開かせる相手を決めるのに使う。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 宣言があるファイルの URI。
    pub(super) fn uri(&self) -> &Uri {
        &self.uri
    }

    /// そのファイルの中で、宣言の名前が置かれている位置。
    pub(super) fn position(&self) -> SourcePosition {
        self.position
    }
}

/// typeDefinition に尋ねた結果。
///
/// **「取れなかった」を 1 つにまとめない。** どれなのかで**利用者が次に試すことが違う**
/// （サーバを替える / そのコードベースの見せ方を直す / dryguard 側の穴）
/// (`rules/architecture.md`「取れなかったシグナルを既定値で埋めない」)。
#[derive(Debug)]
pub enum TypeDefinitionOutcome {
    /// 宣言の場所が返った。
    Answered(DeclarationSite),
    /// サーバはこの位置の宣言を返さなかった。
    ///
    /// **空配列もここに入れる。** 宣言が本当に無いときと、そのファイルをプロジェクトとして
    /// 見ていないときの両方で空が返るので、材料が取れたことにしない。
    NoAnswer,
    /// 宣言は返ったが、パスとして読めない URI だった。
    Unreadable {
        /// 読めなかった理由。どの URI で落ちたかを持つ。
        cause: UriPathError,
    },
    /// サーバが typeDefinition を提供していない。要求は送っていない。
    NotSupported,
}

/// typeDefinition の応答を、読み取れたかどうかが分かる形にする。
///
/// **返った先頭の 1 件だけを採る。** 宣言が複数返るのは同じ名前が複数箇所で宣言されて
/// いる場合（`declare` の重ね合わせ）で、そこから 1 つを選ぶ材料はこの層に無い。
pub(super) fn outcome_of(answered: &GotoDefinitionResponse) -> TypeDefinitionOutcome {
    let site = match answered {
        GotoDefinitionResponse::Scalar(location) => Some(site_of(location)),
        GotoDefinitionResponse::Array(locations) => locations.first().map(site_of),
        GotoDefinitionResponse::Link(links) => links.first().map(site_of_link),
    };

    match site {
        Some(Ok(site)) => TypeDefinitionOutcome::Answered(site),
        Some(Err(cause)) => TypeDefinitionOutcome::Unreadable { cause },
        None => TypeDefinitionOutcome::NoAnswer,
    }
}

/// 宣言 1 件を、開かせて尋ね直せる場所にする。
///
/// # Errors
///
/// URI をパスとして読めないとき。
fn site_of(location: &Location) -> Result<DeclarationSite, UriPathError> {
    Ok(DeclarationSite {
        uri: location.uri.clone(),
        path: uri::path_of(&location.uri)?,
        position: SourcePosition::from_lsp_position(location.range.start),
    })
}

/// `LocationLink` の形で返った宣言 1 件を、開かせて尋ね直せる場所にする。
///
/// **`target_selection_range` を採る。** `target_range` は宣言の本体まで覆っており、
/// 先頭は `export` や `type` の綴りになる。そこを指して尋ね直しても名前の型は返らない。
///
/// # Errors
///
/// URI をパスとして読めないとき。
fn site_of_link(link: &LocationLink) -> Result<DeclarationSite, UriPathError> {
    Ok(DeclarationSite {
        uri: link.target_uri.clone(),
        path: uri::path_of(&link.target_uri)?,
        position: SourcePosition::from_lsp_position(link.target_selection_range.start),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    use lsp_types::{Position, Range};

    /// サーバが返す形の宣言 1 件。
    fn location(uri: &str, line: u32, character: u32) -> Location {
        Location {
            uri: Uri::from_str(uri).expect("テストが渡す文字列は URI として読める"),
            range: Range::new(
                Position::new(line, character),
                Position::new(line, character + 1),
            ),
        }
    }

    /// 読み取れた宣言の場所。読み取れていなければテストを落とす。
    fn site_of_response(answered: &GotoDefinitionResponse) -> DeclarationSite {
        match outcome_of(answered) {
            TypeDefinitionOutcome::Answered(site) => site,
            other => panic!("宣言の場所を読み取れる: {other:?}"),
        }
    }

    #[test]
    fn test_type_definition_outcome_of_a_single_location_is_the_file_it_names() {
        let answered =
            GotoDefinitionResponse::Scalar(location("file:///repo/src/billing/money.ts", 0, 12));

        assert_eq!(
            site_of_response(&answered).path(),
            Path::new("/repo/src/billing/money.ts")
        );
    }

    #[test]
    fn test_type_definition_outcome_of_a_location_keeps_the_position_the_server_named() {
        // 宣言の位置は、そこを指して尋ね直すために持つ。落とすとエイリアスの右辺が取れない
        let answered =
            GotoDefinitionResponse::Scalar(location("file:///repo/src/billing/money.ts", 4, 12));

        let site = site_of_response(&answered);

        assert_eq!(site.position().line().get(), 5);
        assert_eq!(site.position().character(), 12);
    }

    #[test]
    fn test_type_definition_outcome_of_several_locations_is_the_first_of_them() {
        // 対照として 2 件目を別のファイルにする。畳む先を決めていないと、
        // 同じ入力で別のファイルが返りうる
        let answered = GotoDefinitionResponse::Array(vec![
            location("file:///repo/src/billing/money.ts", 0, 12),
            location("file:///repo/src/report/money.ts", 0, 12),
        ]);

        assert_eq!(
            site_of_response(&answered).path(),
            Path::new("/repo/src/billing/money.ts")
        );
    }

    #[test]
    fn test_type_definition_outcome_of_a_link_points_at_the_name_not_the_whole_declaration() {
        // `target_range` は `export type Amount = number;` の行頭から始まる。
        // そこを指して尋ね直すと、`export` の綴りの上を指すことになる
        let link = LocationLink {
            origin_selection_range: None,
            target_uri: Uri::from_str("file:///repo/src/billing/money.ts")
                .expect("テストが渡す文字列は URI として読める"),
            target_range: Range::new(Position::new(3, 0), Position::new(3, 28)),
            target_selection_range: Range::new(Position::new(3, 12), Position::new(3, 18)),
        };
        let answered = GotoDefinitionResponse::Link(vec![link]);

        assert_eq!(site_of_response(&answered).position().character(), 12);
    }

    #[test]
    fn test_type_definition_outcome_of_no_locations_is_not_an_empty_answer() {
        // 空配列は「宣言が無い」と「プロジェクトとして見ていない」の両方で返る。
        // 材料が取れたことにすると、後段は「解決した結果その綴りだった」と読む
        assert!(matches!(
            outcome_of(&GotoDefinitionResponse::Array(Vec::new())),
            TypeDefinitionOutcome::NoAnswer
        ));
    }

    #[test]
    fn test_type_definition_outcome_of_an_unreadable_uri_is_not_an_absent_answer() {
        // 対照は上のテスト。どちらも「宣言に届かない」だが、直す先が違う
        let answered = GotoDefinitionResponse::Scalar(location("untitled:Untitled-1", 0, 0));

        assert!(matches!(
            outcome_of(&answered),
            TypeDefinitionOutcome::Unreadable { .. }
        ));
    }
}
