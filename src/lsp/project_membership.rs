//! projectInfo の応答から、サーバがそのファイルに割り当てたプロジェクトを取り出す。
//!
//! **サーバとの往復そのものは `connection` が持つ。** ここにあるのは受け取った応答を
//! こちらが読める形へ直す変換だけなので、サーバを起動せずに確かめられる
//! (rules/tdd.md「`lsp` は『応答を受け取ってから先』を切り出す」)。

use std::path::PathBuf;

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
pub enum ProjectMembershipOutcome {
    /// サーバがそのファイルのプロジェクトを名指した。
    ///
    /// **印のものとは限らない。** 範囲から外れたファイルには、サーバがその場で
    /// 組み立てたプロジェクトが割り当たる。印のものかを決めるのは
    /// [`super::workspace::ProjectRoot::is_marked_project`]。
    Named {
        /// サーバが名乗ったプロジェクトの綴り。
        project: PathBuf,
    },
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
    fn test_project_of_an_inferred_project_is_the_name_the_server_gave_it() {
        // 対照は上のテスト。**ここで弾かない。** 印のものかを決めるのは
        // 印の一覧を持つ側で、ここは綴りを読むところまで
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
}
