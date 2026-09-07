//! projectInfo の応答から、サーバがそのファイルに割り当てたプロジェクトを読む。
//!
//! **応答を読むところと、実在を確かめるところを分ける。** [`project_of`] は受け取った
//! 応答を綴りへ直すだけの純粋関数で、サーバもファイルシステムも要らずに確かめられる
//! (rules/tdd.md「`lsp` は『応答を受け取ってから先』を切り出す」)。
//! 実在を見るのは [`membership_of`] の側だけ。**混ぜると、同じ応答が置かれた環境で
//! 別の答えになる**（記録した応答を別の木で読み直すと範囲外に倒れる）。

use std::path::{Path, PathBuf};

use serde_json::Value;

/// tsserver への要求をそのまま通す、typescript-language-server の口。
///
/// **サーバが広告したときだけ送る。** LSP の標準にはプロジェクト所属を返す要求が無く、
/// これを提供するかどうかがそのまま「尋ねる手立てがあるか」になる
/// （`capabilities.executeCommandProvider.commands` に載る）。
pub(super) const TSSERVER_REQUEST_COMMAND: &str = "typescript.tsserverRequest";

/// 上の口を通して尋ねる tsserver のコマンド。ファイル 1 つの所属プロジェクトを返す。
pub(super) const PROJECT_INFO_COMMAND: &str = "projectInfo";

/// そのファイルがどのプロジェクトの一員として扱われているかを尋ねた結果。
///
/// **「印の下にある」とは別の問い。** 印は綴りだけで決まるが、こちらは
/// **サーバが実際にどう扱っているか**で、範囲を絞る宣言（`include` / `exclude` /
/// `files`）の効果まで含んだ答えになる（`rules/naming.md`「`project root` と
/// `project membership` を混ぜない」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProjectMembershipOutcome {
    /// 設定ファイルが決めたプロジェクトの一員として扱われている。
    ///
    /// **印の名前とは限らない。** solution-style の木では、根の `tsconfig.json` が
    /// `references` で指す `tsconfig.app.json` のような綴りが返る（実測）。
    /// **その設定ファイルが範囲に入れている以上、参照元はその範囲で揃う。**
    Configured {
        /// サーバが名乗った設定ファイル。
        config: PathBuf,
    },
    /// サーバがその場で組み立てたプロジェクトに入れられている。
    ///
    /// 設定ファイルの範囲から外れたか、そもそも設定ファイルが無いかのどちらか。
    /// どちらでも、参照元は開かせたファイルとその import 先からしか集まらない。
    Inferred,
    /// サーバに尋ねる手立てが無い。要求は送っていない。
    NotSupported,
}

/// projectInfo の応答から、割り当てられたプロジェクトの綴りを取り出す。
///
/// `result` は `workspace/executeCommand` が返した tsserver の応答そのもの。
/// 綴りを読み取れなければ `None`（応答が読めなかったことは呼び出し側が失敗として扱う）。
///
/// **成否のフラグを見ずに、綴りが取れたかで判断する。** 失敗した応答は本文を持たないので
/// 綴りも取れず、`success` を別に読んでも同じ答えにしかならない。
pub(super) fn project_of(result: &Value) -> Option<PathBuf> {
    let named = result.get("body")?.get("configFileName")?.as_str()?;

    Some(PathBuf::from(named))
}

/// 名乗られたプロジェクトが、設定ファイルのものか、サーバが組み立てたものか。
///
/// `project` は [`project_of`] が取り出した綴り。
///
/// **実在するファイルを名乗ったかで分ける。** tsserver が自分で組み立てたプロジェクトは
/// `/dev/null/inferredProject1*` のような、ファイルとして存在しない綴りを名乗る。
///
/// **Why not（印のファイル名と突き合わせる）**: solution-style の木では
/// `tsconfig.app.json` のような印にない綴りが返り、**範囲に入っているファイルまで
/// 範囲外と答えてしまう**（参照元が揃っているのに落とすことになる）。
///
/// **Why not（`/dev/null/inferredProject1*` の綴りで見分ける）**: tsserver の
/// 内部表現なので、こちらの語彙に無い綴りに判定をぶら下げることになる。
pub(super) fn membership_of(project: &Path) -> ProjectMembershipOutcome {
    if !project.is_file() {
        return ProjectMembershipOutcome::Inferred;
    }

    ProjectMembershipOutcome::Configured {
        config: project.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    use crate::test_support::repository_path;

    /// tsserver が返す形の応答 1 通分。本文の中身だけをテストごとに差し替える。
    fn response(body: Value) -> Value {
        json!({
            "seq": 0,
            "type": "response",
            "command": "projectInfo",
            "request_seq": 1,
            "success": true,
            "body": body,
        })
    }

    #[test]
    fn test_project_of_a_configured_project_is_the_config_file_it_names() {
        let result = response(json!({
            "configFileName": "/repo/src/tsconfig.json",
            "languageServiceDisabled": false,
        }));

        assert_eq!(
            project_of(&result),
            Some(PathBuf::from("/repo/src/tsconfig.json"))
        );
    }

    #[test]
    fn test_project_of_a_project_the_server_built_itself_is_the_name_it_gave() {
        // 対照は上のテスト。**ここでは弾かない。** 実在を見るのは `membership_of` の
        // 側で、こちらは置かれた環境に答えが左右されない
        let result = response(json!({
            "configFileName": "/dev/null/inferredProject1*",
            "languageServiceDisabled": false,
        }));

        assert_eq!(
            project_of(&result),
            Some(PathBuf::from("/dev/null/inferredProject1*"))
        );
    }

    #[test]
    fn test_project_of_a_response_without_a_body_is_not_read() {
        // 失敗した応答は本文を持たない。空の綴りを返すと、呼び出し側は
        // 「読めなかった」と「プロジェクトの名前が空」を区別できない
        let result = json!({
            "seq": 0,
            "type": "response",
            "command": "projectInfo",
            "request_seq": 1,
            "success": false,
            "message": "no project",
        });

        assert_eq!(project_of(&result), None);
    }

    #[test]
    fn test_project_of_a_body_without_a_config_file_name_is_not_read() {
        // 対照は上のテスト。本文はあるが、こちらが読む項目だけが無い形
        let result = response(json!({ "languageServiceDisabled": false }));

        assert_eq!(project_of(&result), None);
    }

    #[test]
    fn test_project_of_a_config_file_name_that_is_not_text_is_not_read() {
        let result = response(json!({ "configFileName": 12, "languageServiceDisabled": false }));

        assert_eq!(project_of(&result), None);
    }

    #[test]
    fn test_membership_of_an_existing_config_file_is_a_configured_project() {
        let config = repository_path("tests/fixtures/references/src/tsconfig.json");

        assert_eq!(
            membership_of(&config),
            ProjectMembershipOutcome::Configured {
                config: config.clone()
            }
        );
    }

    #[test]
    fn test_membership_of_a_config_file_not_named_by_a_marker_is_still_a_configured_project() {
        // solution-style の木では、根の tsconfig.json が references で指す
        // tsconfig.app.json が返る。**印の名前で絞ると、範囲に入っているファイルまで
        // 範囲外と答える**（参照元が揃っているのに落とすことになる）
        let config = repository_path("tests/fixtures/solution-project/tsconfig.app.json");

        assert_eq!(
            membership_of(&config),
            ProjectMembershipOutcome::Configured {
                config: config.clone()
            }
        );
    }

    #[test]
    fn test_membership_of_a_project_the_server_built_itself_is_an_inferred_project() {
        // 対照は上の 2 つ。tsserver が範囲外のファイルへ割り当てる綴りで、
        // `/dev/null` はディレクトリではないので実在しえない
        let membership = membership_of(Path::new("/dev/null/inferredProject1*"));

        assert_eq!(membership, ProjectMembershipOutcome::Inferred);
    }
}
