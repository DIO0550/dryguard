//! hover の応答から、型の綴りを取り出す。
//!
//! **サーバとの往復そのものは `connection` が持つ。** ここにあるのは受け取った応答を
//! こちらが読める形へ直す変換だけなので、サーバを起動せずに確かめられる
//! (rules/tdd.md「`lsp` は『応答を受け取ってから先』を切り出す」)。

use lsp_types::{Hover, HoverContents};

/// マークダウンのコードフェンスの印。
const FENCE: &str = "```";

/// hover に尋ねた結果。
///
/// **「取れなかった」を 1 つにまとめない。** 3 つのどれなのかで**利用者が次に試すことが
/// 違う**（サーバを替える / その位置には型が無い / dryguard 側が読めていない）
/// (`rules/architecture.md`「取れなかったシグナルを既定値で埋めない」)。
///
/// `Option<String>` で返していたときは、サーバが黙っているのとこちらが読めていないのが
/// 同じ `None` になり、**後段も読者も区別できなかった**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HoverOutcome {
    /// 型の綴りが返った。**正規化前**（`rules/naming.md` の `signature text`）。
    Answered(String),
    /// サーバはその位置に答えを持たない。名前でない位置を指した場合がこれ。
    NoAnswer,
    /// 答えは返ったが、dryguard が読める形ではない。
    ///
    /// コードフェンスが無い（サーバが散文で返した）か、`MarkedString`
    /// （LSP 3.15 で非推奨になった形）で返ってきたか。**どちらもこちら側の穴**で、
    /// サーバは答えを持っている。
    Unreadable,
    /// サーバが hover を提供していない。要求は送っていない。
    NotSupported,
}

/// hover の応答を、読み取れたかどうかが分かる形にする。
///
/// 綴りの正規化（引数名を落とすなど）は `semantics` が行う。ここは応答の形を
/// 読むところまで。
pub(super) fn outcome_of(hover: &Hover) -> HoverOutcome {
    let HoverContents::Markup(content) = &hover.contents else {
        // `MarkedString` は LSP 3.15 で非推奨になった形。今つないでいるサーバは
        // どれも `Markup` を返すので、読める形は要る相手が現れたときに足す。
        // **黙って「答えが無かった」にはしない。**
        return HoverOutcome::Unreadable;
    };

    match fenced_text_of(&content.value) {
        Some(signature_text) => HoverOutcome::Answered(signature_text),
        None => HoverOutcome::Unreadable,
    }
}

/// 最初のコードフェンスの中身。中身が無ければ `None`。
///
/// **フェンスの言語名は見ない。** typescript-language-server は `typescript`、
/// rust-analyzer は `rust` と書くので、名前で選ぶとサーバごとの一覧を持つことになる。
/// どちらも型の綴りを最初のフェンスに置き、続くフェンスは doc コメントの中身になる。
///
/// 改行は畳まずにそのまま残す。TS のサーバはオブジェクト型リテラルを複数行に展開して
/// 返す（`<T extends {\n    id: string;\n}, U>`）が、**どう畳むかは綴りを読む側が決める。**
fn fenced_text_of(markdown: &str) -> Option<String> {
    let mut lines = markdown
        .lines()
        .skip_while(|line| !line.trim_start().starts_with(FENCE));

    // 開きのフェンスの行そのものは中身ではない。無ければフェンスが 1 つも無かった。
    lines.next()?;

    let body: Vec<&str> = lines
        .take_while(|line| !line.trim_start().starts_with(FENCE))
        .collect();

    if body.is_empty() {
        return None;
    }
    Some(body.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{MarkedString, MarkupContent, MarkupKind};

    /// サーバが返す形の hover。
    fn markdown_hover(value: &str) -> Hover {
        Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: value.to_owned(),
            }),
            range: None,
        }
    }

    #[test]
    fn test_hover_outcome_of_a_fenced_answer_is_the_text_inside_the_fence() {
        // typescript-language-server が実際に返す形（前後に空行が付く）
        let hover = markdown_hover("\n```typescript\nfunction decl(a: string): number\n```\n");

        assert_eq!(
            outcome_of(&hover),
            HoverOutcome::Answered("function decl(a: string): number".to_owned())
        );
    }

    #[test]
    fn test_hover_outcome_of_a_fence_broken_over_lines_keeps_every_line() {
        // オブジェクト型リテラルはサーバが複数行に展開して返す。1 行目で打ち切ると
        // 引数リストごと落ちる
        let hover = markdown_hover(
            "```typescript\nfunction constrained<T extends {\n    id: string;\n}, U>(a: T, b: U): [T, U]\n```",
        );

        assert_eq!(
            outcome_of(&hover),
            HoverOutcome::Answered(
                "function constrained<T extends {\n    id: string;\n}, U>(a: T, b: U): [T, U]"
                    .to_owned()
            )
        );
    }

    #[test]
    fn test_hover_outcome_of_an_answer_with_a_doc_comment_stops_at_the_first_fence() {
        // doc コメントを持つ関数では 2 つ目以降のフェンスが続く。綴りは最初のフェンス
        let hover = markdown_hover(
            "```typescript\nfunction decl(a: string): number\n```\n---\n```typescript\n説明\n```",
        );

        assert_eq!(
            outcome_of(&hover),
            HoverOutcome::Answered("function decl(a: string): number".to_owned())
        );
    }

    #[test]
    fn test_hover_outcome_of_a_fence_without_a_language_name_is_still_read() {
        // 言語名で選ぶとサーバごとの一覧が要る。ここでは最初のフェンスの中身を採る
        let hover = markdown_hover("```\nfn decl(a: &str) -> usize\n```");

        assert_eq!(
            outcome_of(&hover),
            HoverOutcome::Answered("fn decl(a: &str) -> usize".to_owned())
        );
    }

    #[test]
    fn test_hover_outcome_of_an_answer_without_a_fence_is_not_read() {
        // 対照は上のテスト。同じ文からフェンスだけを外している
        let hover = markdown_hover("function decl(a: string): number");

        assert_eq!(outcome_of(&hover), HoverOutcome::Unreadable);
    }

    #[test]
    fn test_hover_outcome_of_an_empty_fence_is_not_read() {
        // 空文字列を綴りとして返すと、後段は「型が空だった」と読む
        let hover = markdown_hover("```typescript\n```");

        assert_eq!(outcome_of(&hover), HoverOutcome::Unreadable);
    }

    #[test]
    fn test_hover_outcome_of_a_deprecated_content_form_is_unreadable_not_absent() {
        // `MarkedString` は読めないが、サーバは答えを持っている。`NoAnswer` にすると
        // 「その位置に型が無い」と区別が付かず、こちら側の穴が見えなくなる
        let hover = Hover {
            contents: HoverContents::Scalar(MarkedString::String(
                "function decl(a: string): number".to_owned(),
            )),
            range: None,
        };

        assert_eq!(outcome_of(&hover), HoverOutcome::Unreadable);
    }
}
