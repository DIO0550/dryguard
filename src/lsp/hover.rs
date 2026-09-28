//! hover の応答から、型の綴りを取り出す。
//!
//! **サーバとの往復そのものは `connection` が持つ。** ここにあるのは受け取った応答を
//! こちらが読める形へ直す変換だけなので、サーバを起動せずに確かめられる
//! (rules/tdd.md「`lsp` は『応答を受け取ってから先』を切り出す」)。

use super::ServerLanguage;
use lsp_types::{Hover, HoverContents};

/// マークダウンのコードフェンスの印。
const FENCE: &str = "```";

/// hover が返した綴りそのもの。**正規化前**（`rules/naming.md` の `signature text`）。
///
/// **素の `String` で持ち回らない。** 正規化後の
/// [`crate::semantics::type_signature::TypeSignature`] と混ぜても型では止まらず、
/// `rules/naming.md`「`signature text` と `type signature` を混ぜない」が
/// 文の上にしか無い状態になる（`payload` を `lsp::message` の `Payload` に
/// したのと同じ形）。
///
/// **Why（宣言の綴りもこの型で受ける）**: hover は関数のシグネチャだけでなく
/// 型エイリアスの宣言（`type Amount = number`）にも答える。どちらも
/// 「hover が返した綴り・正規化前」で、分けるには [`HoverOutcome`] 自体を割ることになる。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureText(String);

impl SignatureText {
    /// hover が返した綴りから作る。中身が空白しか無ければ `None`。
    ///
    /// **空の綴りを作れないようにする。** 通すと後段が「型が空だった」と読むが、
    /// 実際には**サーバの答えをこちらが読み取れていない**
    /// (`rules/coding.md`「生成時に検証し、不正な値を存在させない」)。
    pub fn new(text: String) -> Option<Self> {
        if text.trim().is_empty() {
            return None;
        }
        Some(Self(text))
    }

    /// 綴りそのもの。正規化する側が読む。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

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
    /// 型の綴りが返った。
    Answered(SignatureText),
    /// サーバはその位置に答えを持たない。名前でない位置を指した場合がこれ。
    NoAnswer,
    /// 答えは返ったが、dryguard が読める形ではない。
    ///
    /// コードフェンスが無い（サーバが散文で返した）か、`MarkedString`
    /// （LSP 3.15 で非推奨になった形）で返ってきたか。**どちらもこちら側の穴**で、
    /// サーバは答えを持っている。
    Unreadable,
    /// 尋ねるたびにサーバが作業を始めるので、落ち着いた答えを受け取れなかった。
    ///
    /// **途中の綴りを答えとして返さない。** プロジェクトの読み込み前の hover は
    /// 推論された型に `any` を綴って返す（typescript-language-server 6.0.0 で実測）。
    /// `any` は型として読める綴りなので、そのまま採ると比較まで進み、
    /// **中身の違う 2 つが「単一化可能」に出る**（偽陽性）
    /// (`rules/architecture.md`「取れなかったシグナルを既定値で埋めない」)。
    ///
    /// **待ち切っても `any` が返る場合は含まない。** 本当に `any` の関数は書けるので、
    /// 落ち着いた後の `any` は[`HoverOutcome::Answered`] のままにする。
    ServerStillWorking,
    /// サーバが hover を提供していない。要求は送っていない。
    NotSupported,
}

/// hover の応答を、読み取れたかどうかが分かる形にする。
///
/// 綴りの正規化（引数名を落とすなど）は `semantics` が行う。ここは応答の形を
/// 読むところまで。
#[cfg(test)]
pub(super) fn outcome_of(hover: &Hover) -> HoverOutcome {
    outcome_for_language(hover, ServerLanguage::TypeScript)
}

/// サーバの hover レイアウトに応じてシグネチャを読む。
pub(super) fn outcome_for_language(hover: &Hover, language: ServerLanguage) -> HoverOutcome {
    let HoverContents::Markup(content) = &hover.contents else {
        // `MarkedString` は LSP 3.15 で非推奨になった形。今つないでいるサーバは
        // どれも `Markup` を返すので、読める形は要る相手が現れたときに足す。
        // **黙って「答えが無かった」にはしない。**
        return HoverOutcome::Unreadable;
    };

    let text = match language {
        ServerLanguage::TypeScript => fenced_text_of(&content.value, 0),
        ServerLanguage::Rust => {
            fenced_text_of(&content.value, 1).or_else(|| fenced_text_of(&content.value, 0))
        }
    };
    match text.and_then(SignatureText::new) {
        Some(signature_text) => HoverOutcome::Answered(signature_text),
        None => HoverOutcome::Unreadable,
    }
}

/// 指定したコードフェンスの中身。無ければ `None`。
///
/// **フェンスの言語名は見ない。** typescript-language-server は `typescript`、
/// rust-analyzer は `rust` と書くので、名前で選ばない。
///
/// 改行は畳まずにそのまま残す。TS のサーバはオブジェクト型リテラルを複数行に展開して
/// 返す（`<T extends {\n    id: string;\n}, U>`）が、**どう畳むかは綴りを読む側が決める。**
fn fenced_text_of(markdown: &str, index: usize) -> Option<String> {
    // フェンスの開始と終了を数えて、対象の開始位置へ進む。
    let mut fences = markdown
        .lines()
        .enumerate()
        .filter(|(_, line)| line.trim_start().starts_with(FENCE));
    let (start, _) = fences.nth(index * 2)?;
    let (end, _) = fences.next()?;

    // 中身の検証は [`SignatureText::new`] が持つ。
    let body: Vec<&str> = markdown
        .lines()
        .skip(start + 1)
        .take(end - start - 1)
        .collect();

    Some(body.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{MarkedString, MarkupContent, MarkupKind};

    use crate::test_support::signature_text;

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
            HoverOutcome::Answered(signature_text("function decl(a: string): number"))
        );
    }

    #[test]
    fn test_rust_analyzer_hover_reads_the_declaration_after_the_crate_name() {
        let hover = markdown_hover(
            "```rust\ndryguard_fixture\n```\n\n```rust\npub fn first(value: i32) -> i32\n```",
        );

        assert_eq!(
            outcome_for_language(&hover, ServerLanguage::Rust),
            HoverOutcome::Answered(signature_text("pub fn first(value: i32) -> i32"))
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
            HoverOutcome::Answered(signature_text(
                "function constrained<T extends {\n    id: string;\n}, U>(a: T, b: U): [T, U]"
            ))
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
            HoverOutcome::Answered(signature_text("function decl(a: string): number"))
        );
    }

    #[test]
    fn test_hover_outcome_of_a_fence_without_a_language_name_is_still_read() {
        // 言語名で選ぶとサーバごとの一覧が要る。ここでは最初のフェンスの中身を採る
        let hover = markdown_hover("```\nfn decl(a: &str) -> usize\n```");

        assert_eq!(
            outcome_of(&hover),
            HoverOutcome::Answered(signature_text("fn decl(a: &str) -> usize"))
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
    fn test_hover_outcome_of_a_fence_holding_only_a_blank_line_is_not_read() {
        // 対照は上のテスト。行が 1 つある分だけフェンスの「中身が無い」判定をすり抜ける
        let hover = markdown_hover("```typescript\n\n```");

        assert_eq!(outcome_of(&hover), HoverOutcome::Unreadable);
    }

    #[test]
    fn test_signature_text_of_a_spelling_that_is_only_whitespace_cannot_be_made() {
        // 対照は下のテスト。同じ空白を綴りの前後に付けただけの違い
        assert_eq!(SignatureText::new("   ".to_owned()), None);
    }

    #[test]
    fn test_signature_text_of_a_spelling_padded_with_whitespace_keeps_the_padding() {
        // 前後の空白を落とすと、サーバが返した綴りと持っている綴りが食い違う。
        // どう畳むかは綴りを読む側が決める
        let padded = SignatureText::new("  function decl(): void  ".to_owned());

        assert_eq!(
            padded.map(|text| text.as_str().to_owned()),
            Some("  function decl(): void  ".to_owned())
        );
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
