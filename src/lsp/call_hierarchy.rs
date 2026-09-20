//! callHierarchy の応答から、そのチャンクが呼んでいるファイルを取り出す。
//!
//! **サーバとの往復そのものは `connection` が持つ。** ここにあるのは受け取った応答を
//! こちらが読める形へ直す変換だけなので、サーバを起動せずに確かめられる
//! (rules/tdd.md「`lsp` は『応答を受け取ってから先』を切り出す」)。
//!
//! 呼び出し先を尋ねるには 2 往復要る。`textDocument/prepareCallHierarchy` で
//! **起点（`call hierarchy item`）**を取り、それを `callHierarchy/outgoingCalls` へ渡す。
//! 起点を決めるところと、返った呼び出し先を読むところが、それぞれこのモジュールの
//! 純粋関数になる。

use std::path::PathBuf;

use lsp_types::{CallHierarchyItem, CallHierarchyOutgoingCall};

use super::uri::{self, UriPathError};

/// 呼び出し先を尋ねた結果。
///
/// **「取れなかった」を 1 つにまとめない。** どれなのかで**利用者が次に試すことが違う**
/// （サーバを替える / 尋ねる位置を直す / そのチャンクは本当に何も呼んでいない）
/// (`rules/architecture.md`「取れなかったシグナルを既定値で埋めない」)。
///
/// **Why not（要求名で `CallHierarchyOutcome` と呼ぶ）**: `callHierarchy` は
/// incoming と outgoing の 2 方向を持つので、要求名で呼ぶと**呼び出し元も返る型**に読める。
/// 返すもの（`callee`）を名前に出す（`rules/naming.md`「名前と実体を一致させる」）。
#[derive(Debug)]
pub enum CalleesOutcome {
    /// 呼び出し先のファイルが返った。
    ///
    /// **同じファイルの別の関数を呼んでいれば 2 つ並ぶ。** ドメインごとの件数が
    /// 判定の根拠になるので、ファイル単位で畳まない。
    Answered(Vec<PathBuf>),
    /// その位置に呼び出し関係の起点が無かった。
    ///
    /// **`NoCallees` と分ける。** どちらも呼び出し先が返らないが、こちらは
    /// **尋ねる位置かサーバの側**の話（dryguard が指した位置をサーバが起点として
    /// 認めなかった）、あちらは**対象のコードの側**の話（本当に何も呼んでいない）。
    NoItem,
    /// 起点が 2 つ以上返り、どれがそのチャンクのものか決められなかった。
    ///
    /// **先頭を採らない。** 先頭がそのチャンクの起点とは限らず、**別の宣言の
    /// 呼び出し先をこのチャンクのものとして数える**ことになる（偽陽性）。
    SeveralItems {
        /// 返った起点の数。どれだけ曖昧だったかを出すのに要る。
        count: usize,
    },
    /// 起点はあるが、呼び出し先が 1 件も返らなかった。
    ///
    /// **空配列もここに入れる。** tsserver はプロジェクトを読み終える前の
    /// 問い合わせにも空配列を返すので、材料が取れたことにすると、後段は 0 件を
    /// 「何にも依存していない」と読む（`super::references::ReferencesOutcome::NoAnswer`
    /// と同じ形）。
    NoCallees,
    /// 呼び出し先は返ったが、パスとして読めない URI が混じっていた。
    ///
    /// **読めた分だけを返さない。** ドメインごとの件数が黙って目減りし、
    /// 「呼び出し先が少ない」のか「読めていない」のかを後段が区別できなくなる。
    Unreadable {
        /// 読めなかった理由。どの URI で落ちたかを持つ。
        cause: UriPathError,
    },
    /// 尋ねるたびにサーバが作業を始めるので、落ち着いた答えを受け取れなかった。
    ///
    /// **途中の答えを最終的なシグナルとして返さない。** 作業中の答えは
    /// まだ見ていないファイルの分が抜けており、依存先の広がりが実際より狭く出る
    /// （`rules/architecture.md`「取れなかったシグナルを既定値で埋めない」）。
    ServerStillWorking,
    /// サーバが callHierarchy を提供していない。要求は送っていない。
    NotSupported,
}

/// 呼び出し先を尋ねる起点。決められなければ、その理由がそのまま答えになる。
///
/// **`Option` にしない。** 起点が無いのと 2 つ以上あるのとでは利用者が直す先が違うので、
/// 決められなかった理由を落とさずに持つ
/// (`rules/coding.md`「エラー型は原因ごとにバリアントを分ける」)。
#[derive(Debug)]
pub(super) enum CallHierarchyStart<'a> {
    /// 起点が 1 つに決まった。
    Item(&'a CallHierarchyItem),
    /// 決められなかった。持っているのが、そのまま呼び出し先の答えになる。
    Undecidable(CalleesOutcome),
}

/// prepareCallHierarchy の応答から、呼び出し先を尋ねる起点を決める。
///
/// `items` はサーバが返した起点。1 つだけのときに限って尋ねに行く。
pub(super) fn start_of(items: &[CallHierarchyItem]) -> CallHierarchyStart<'_> {
    let [item] = items else {
        if items.is_empty() {
            return CallHierarchyStart::Undecidable(CalleesOutcome::NoItem);
        }
        return CallHierarchyStart::Undecidable(CalleesOutcome::SeveralItems {
            count: items.len(),
        });
    };

    CallHierarchyStart::Item(item)
}

/// outgoingCalls の応答を、読み取れたかどうかが分かる形にする。
///
/// `calls` はサーバが返した呼び出し先。返す順はサーバが返した順のまま。
///
/// **呼び出し箇所の数（`from_ranges`）は数えない。** 応答は**呼ばれている相手 1 つにつき
/// 1 件**で、同じ相手を 2 回呼んでいれば範囲が 2 つ載る。同じ相手を 2 回呼ぶことは
/// 「その相手に依存している」という 1 つの事実で、2 倍の証拠ではない
/// （`semantics::domain::DomainCounts::jaccard` が件数で重み付けしないのと同じ考え）。
///
/// ドメインの導出（どのディレクトリに属するか）は `semantics` が行う。ここは応答の形を
/// 読むところまで。
pub(super) fn outcome_of(calls: &[CallHierarchyOutgoingCall]) -> CalleesOutcome {
    if calls.is_empty() {
        return CalleesOutcome::NoCallees;
    }

    let mut paths = Vec::with_capacity(calls.len());

    for call in calls {
        match uri::path_of(&call.to.uri) {
            Ok(path) => paths.push(path),
            Err(cause) => return CalleesOutcome::Unreadable { cause },
        }
    }

    CalleesOutcome::Answered(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::str::FromStr;

    use lsp_types::{Position, Range, SymbolKind, Uri};

    /// サーバが返す形の起点 1 つ。
    ///
    /// 範囲は使わない（起点をそのまま `outgoingCalls` へ渡すだけ）ので、行頭を指す
    /// 最小の形にする。
    fn item(uri: &str) -> CallHierarchyItem {
        CallHierarchyItem {
            name: "applyDiscount".to_owned(),
            kind: SymbolKind::FUNCTION,
            tags: None,
            detail: None,
            uri: Uri::from_str(uri).expect("テストが渡す文字列は URI として読める"),
            range: Range::new(Position::new(0, 0), Position::new(0, 0)),
            selection_range: Range::new(Position::new(0, 0), Position::new(0, 0)),
            data: None,
        }
    }

    /// サーバが返す形の呼び出し先 1 件。
    ///
    /// `call_sites` は呼び出し箇所の数。**数えない**ことを確かめるテストが使う。
    fn outgoing_call(uri: &str, call_sites: usize) -> CallHierarchyOutgoingCall {
        CallHierarchyOutgoingCall {
            to: item(uri),
            from_ranges: (0..call_sites)
                .map(|line| {
                    let line = u32::try_from(line).expect("テストが渡す件数は u32 に収まる");
                    Range::new(Position::new(line, 0), Position::new(line, 0))
                })
                .collect(),
        }
    }

    /// 読み取れた呼び出し先のパス。読み取れていなければテストを落とす。
    fn paths_of(calls: &[CallHierarchyOutgoingCall]) -> Vec<PathBuf> {
        match outcome_of(calls) {
            CalleesOutcome::Answered(paths) => paths,
            other => panic!("呼び出し先を読み取れる: {other:?}"),
        }
    }

    #[test]
    fn test_start_of_one_item_is_that_item() {
        let items = [item("file:///repo/src/billing/invoice.ts")];

        assert!(matches!(
            start_of(&items),
            CallHierarchyStart::Item(started) if started.name == "applyDiscount"
        ));
    }

    #[test]
    fn test_start_of_no_items_is_not_an_absent_callee_set() {
        // 「起点が無い」を `NoCallees` にすると、尋ねる位置が悪かったことと
        // 何も呼んでいないことが同じ答えになる
        assert!(matches!(
            start_of(&[]),
            CallHierarchyStart::Undecidable(CalleesOutcome::NoItem)
        ));
    }

    #[test]
    fn test_start_of_several_items_does_not_take_the_first_one() {
        // 対照は 1 つだけのテスト。先頭を採ると、**別の宣言の呼び出し先**を
        // このチャンクのものとして数える
        let items = [
            item("file:///repo/src/billing/invoice.ts"),
            item("file:///repo/src/inventory/stock.ts"),
        ];

        assert!(matches!(
            start_of(&items),
            CallHierarchyStart::Undecidable(CalleesOutcome::SeveralItems { count: 2 })
        ));
    }

    #[test]
    fn test_callees_outcome_of_calls_into_separate_files_keeps_every_file() {
        let calls = [
            outgoing_call("file:///repo/src/utils/pad.ts", 1),
            outgoing_call("file:///repo/src/utils/round.ts", 1),
        ];

        assert_eq!(
            paths_of(&calls),
            vec![
                PathBuf::from("/repo/src/utils/pad.ts"),
                PathBuf::from("/repo/src/utils/round.ts"),
            ]
        );
    }

    #[test]
    fn test_callees_outcome_of_two_callees_in_one_file_keeps_both() {
        // 対照は上のテスト。同じファイルの別の関数を 2 つ呼んでいる形で、
        // ファイル単位で畳むと 1 件になる。ドメインごとの件数が判定の根拠なので、
        // ここで畳むと数が出せない
        let calls = [
            outgoing_call("file:///repo/src/utils/pad.ts", 1),
            outgoing_call("file:///repo/src/utils/pad.ts", 1),
        ];

        assert_eq!(paths_of(&calls).len(), 2);
    }

    #[test]
    fn test_callees_outcome_of_one_callee_called_twice_is_counted_once() {
        // 対照は上のテスト。相手は 1 つで、呼び出し箇所だけが 2 つある形。
        // `from_ranges` を数えていると 2 件になる
        let calls = [outgoing_call("file:///repo/src/utils/pad.ts", 2)];

        assert_eq!(paths_of(&calls).len(), 1);
    }

    #[test]
    fn test_callees_outcome_of_an_encoded_uri_is_the_path_it_spells() {
        let calls = [outgoing_call("file:///repo/my%20project/pad.ts", 1)];

        assert_eq!(paths_of(&calls), vec![Path::new("/repo/my project/pad.ts")]);
    }

    #[test]
    fn test_callees_outcome_of_no_calls_is_not_an_empty_answer() {
        // 空配列は「何も呼んでいない」と「プロジェクトをまだ読み終えていない」の
        // 両方で返る。Answered(vec![]) にすると、後段が 0 件を材料として読む
        assert!(matches!(outcome_of(&[]), CalleesOutcome::NoCallees));
    }

    #[test]
    fn test_callees_outcome_of_calls_including_an_unreadable_uri_is_not_partially_read() {
        // 対照として読める URI を 1 件置く。読めた分だけ返すと、この入力は
        // 「呼び出し先 1 件」に見え、読めなかったことが数から消える
        let calls = [
            outgoing_call("file:///repo/src/utils/pad.ts", 1),
            outgoing_call("untitled:Untitled-1", 1),
        ];

        assert!(matches!(
            outcome_of(&calls),
            CalleesOutcome::Unreadable { .. }
        ));
    }
}
