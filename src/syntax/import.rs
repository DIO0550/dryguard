//! import 文が指す依存先と、その集合。
//!
//! 指定子を**文字列のまま比べない**。`./pad` と `../utils/pad` は綴りが違うだけで
//! 同じファイルを指しうるので、そのまま比べると共有している依存を「別物」と数える。
//! 依存先が食い違っていると誤って言うのは、このツールが最も損をする外し方
//! （共有ユーティリティに「共通化するな」と言うことになる）。

use std::borrow::Cow;
use std::collections::HashSet;
use std::path::{Component, Path};

use tree_sitter::Node;

use crate::line_number::LineNumber;
use crate::similarity::Similarity;
use crate::syntax::tree::SyntaxTree;

/// 解決済みの依存先。相対指定は importer の位置から畳んである。
///
/// 区切りは常に `/`。プラットフォームの区切りをそのまま持つと、
/// 同じ依存先が OS によって別の値になる。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModulePath(String);

impl ModulePath {
    /// import 指定子を、それを書いているファイルの位置から解決する。
    ///
    /// `specifier` は依存の宣言が依存先として書いている文字列（`from` の後ろ・
    /// `require` の引数）、`importer` はそれを書いているファイル。
    /// 相対指定（`./` / `../`）だけを畳み、パッケージ名は書かれたまま返す
    /// （パッケージ名は importer の位置に依らない）。
    ///
    /// **ファイルシステムは見ない。** 指す先が実在するとは限らず
    /// （拡張子の省略・`tsconfig` のパスエイリアス）、`syntax` は I/O を持てない
    /// (rules/coding.md 禁止事項)。
    pub fn from_specifier(specifier: &str, importer: &Path) -> Self {
        let written = with_forward_separators(specifier);
        if !is_relative(&written) {
            // パッケージ名は importer の位置に依らないので、区切りも直さず綴りのまま残す
            return Self(specifier.to_string());
        }

        let directory = importer.parent().unwrap_or_else(|| Path::new(""));
        Self(folded_path(directory, &written))
    }

    /// 解決済みの依存先そのもの。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// ファイル 1 つ分の、依存している先の集合。
///
/// 空では作れない。import が 1 つも無いことを空の集合として通すと、後段が
/// 「依存先が食い違っている」と「材料が無い」を区別できなくなる
/// (rules/architecture.md「取れなかったシグナルを既定値で埋めない」)。
///
/// **欠けたままでも作れない。** 読み取れなかった宣言が 1 つでもあれば作らない
/// （同じ規約の適用。下の [`ImportSet::from_tree`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportSet(HashSet<ModulePath>);

/// 依存先の集合を作れなかった理由。
///
/// **1 つにまとめない。** 利用者が次にすることが違う（`rules/architecture.md`
/// 「理由は落とさない」）。
///
/// | 理由 | 誰の側の話か | 次にすること |
/// |---|---|---|
/// | [`Self::NoDeclarations`] | 利用者のファイル | そういうファイル |
/// | [`Self::ReboundSpelling`] | 利用者のファイル | このツールでは測れない書き方 |
/// | [`Self::UnreadableDeclaration`] | **dryguard** | 穴として直す |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportsUnavailable {
    /// 依存の宣言が 1 つも書かれていない。
    NoDeclarations,
    /// 宣言はすべて読み取れたが、`require` の綴りが読み込みを指すと言い切れない。
    ///
    /// **[`Self::UnreadableDeclaration`] と混ぜない。** こちらは書いてあるものを
    /// すべて読めていて、測れないのは綴りが曖昧だから。混ぜると、利用者を
    /// dryguard の穴のほうへ向けてしまう。
    ReboundSpelling,
    /// 依存の宣言はあるが、指定子を読み取れなかった。
    ///
    /// **どの行かを持つ。** 宣言は 1 ファイルに複数あるので、「どこかのファイルで
    /// 読み取れなかった」だけでは**直す先も、dryguard の穴を再現する手がかりも
    /// 渡せない**（`rules/coding.md`「バリアントが持つ値は直すために要る情報に限る」）。
    UnreadableDeclaration {
        /// 読み取れなかった宣言が書かれている行。
        line: LineNumber,
    },
}

impl ImportSet {
    /// 構文木の import を集めて、解決済みの依存先の集合にする。
    ///
    /// `tree` は importer の構文木、`importer` はそのファイルの位置。
    ///
    /// # Errors
    ///
    /// 依存の宣言が 1 つも書かれていなければ [`ImportsUnavailable::NoDeclarations`]。
    ///
    /// `require` の綴りが読み込みを指すと言い切れなければ
    /// [`ImportsUnavailable::ReboundSpelling`]。
    ///
    /// **読み取れなかった宣言が 1 つでもあれば** [`ImportsUnavailable::UnreadableDeclaration`]。
    /// 欠けたまま集合を返すと、後段はその重なりを測れた値として読む。1 件落ちるだけで
    /// 重なりは過大にも過小にも動くので、**落ちたことが構造に出ないと区別できない**
    /// (rules/architecture.md「取れなかったシグナルを既定値で埋めない」)。
    pub fn from_tree(tree: &SyntaxTree<'_>, importer: &Path) -> Result<Self, ImportsUnavailable> {
        let specifiers = specifiers_of(tree)?;
        let paths: HashSet<ModulePath> = specifiers
            .iter()
            .map(|specifier| ModulePath::from_specifier(specifier, importer))
            .collect();

        if paths.is_empty() {
            return Err(ImportsUnavailable::NoDeclarations);
        }
        Ok(Self(paths))
    }

    /// 2 つの依存先集合の Jaccard 係数（共通している依存先が、合わせたうちの何割か）。
    ///
    /// これが Phase 0 で唯一のドメインシグナル。Stage 2 を飛ばしているので、
    /// 依存先が同じかどうかはここでしか言えない。
    pub fn jaccard(&self, other: &Self) -> Similarity {
        let shared = self.0.intersection(&other.0).count();
        let combined = self.0.union(&other.0).count();

        Similarity::from_shared_count(shared, combined)
    }
}

/// 依存先を宣言する文の種別。どちらも `source` フィールドに指定子の文字列を持つ。
///
/// `export_statement` を入れるのは `export { pad } from "./pad"` と
/// `export * from "./pad"` のため。`from` を持たない `export` は
/// `source` フィールドを持たないので、同じ規則のまま外れる。
const DEPENDENCY_STATEMENT_KINDS: [&str; 2] = ["import_statement", "export_statement"];

/// `import dep = require("./dep")` の右辺。
///
/// この形は `call_expression` にならないので、呼び出しの規則では拾えない。
/// 名前空間の別名（`import dep = A.B`）は別の種別（`import_alias`）になるため、
/// 種別で見るだけで外れる。
const IMPORT_REQUIRE_CLAUSE_KIND: &str = "import_require_clause";

const CALL_EXPRESSION_KIND: &str = "call_expression";
const MEMBER_EXPRESSION_KIND: &str = "member_expression";
const UNARY_EXPRESSION_KIND: &str = "unary_expression";
const SUBSCRIPT_EXPRESSION_KIND: &str = "subscript_expression";
const IMPORT_SPECIFIER_KIND: &str = "import_specifier";
const EXPORT_SPECIFIER_KIND: &str = "export_specifier";
const EXPORT_STATEMENT_KIND: &str = "export_statement";
const PAIR_KIND: &str = "pair";
const NAMESPACE_EXPORT_KIND: &str = "namespace_export";
const JSX_ATTRIBUTE_KIND: &str = "jsx_attribute";
/// 名前空間つきの名前（`<Widget require:mode="x" />` / `<require:tag />`）。
const JSX_NAMESPACE_NAME_KIND: &str = "jsx_namespace_name";
/// タグの名前を `name` の欄に持つ JSX の要素。
const JSX_ELEMENT_KINDS: [&str; 3] = [
    "jsx_opening_element",
    "jsx_closing_element",
    "jsx_self_closing_element",
];
const NEW_EXPRESSION_KIND: &str = "new_expression";
const COMPUTED_PROPERTY_NAME_KIND: &str = "computed_property_name";
/// 分解して受ける側の欄。書く側（`pair`）と違い、**欄を読む**。
const PAIR_PATTERN_KIND: &str = "pair_pattern";
/// ラベルの名前。文に付ける名前で、値の名前とは別の名前空間にいる。
const STATEMENT_IDENTIFIER_KIND: &str = "statement_identifier";
const ESCAPE_SEQUENCE_KIND: &str = "escape_sequence";
/// テンプレートの置換（`` `title-${locale}` `` の `${locale}`）。綴りが**実行時に決まる**印。
const TEMPLATE_SUBSTITUTION_KIND: &str = "template_substitution";

/// 型を言い当てる式。**型と値の欄を分けていない**ので、どちらの側かは
/// ノードの種別（`type_identifier` か `identifier` か）で見分ける。
const ASSERTION_KINDS: [&str; 2] = ["as_expression", "satisfies_expression"];
/// **値が書かれている子が最初の子**にある包み。**綴りを見る前に剥がす。**
///
/// 括弧、TypeScript の型だけの注記（`require as NodeRequire` / `require!` /
/// `require satisfies NodeRequire` / `require<T>`）、`await`。
/// **値になる子の位置で 2 つに分かれる**（[`WRAPPERS_HOLDING_THE_VALUE_LAST`]）。
///
/// **`await` だけは値をそのまま返すとは限らない**（Promise は解ける）。それでも
/// ここに入れてよいのは、**このモジュールが読むのは書かれた綴りだけ**だから。
/// `await "./dep"` の値は書かれた綴りそのもので、`await somePromise` は綴りを
/// 読み取れない側へ落ちる（どちらに入れても答えが変わらない）。
///
/// **ここから漏れた包みは [`RequireSpelling::Rebound`] へ落ちる。** 落ちた先は
/// 「読み取れない」なので、**値を作らずに測れないと言う**だけで済む
/// (rules/coding.md「列挙で判定を組むときは、漏れの倒れる向きを選ぶ」)。
const WRAPPERS_HOLDING_THE_VALUE_FIRST: [&str; 6] = [
    "parenthesized_expression",
    "as_expression",
    "satisfies_expression",
    "non_null_expression",
    "await_expression",
    "instantiation_expression",
];

/// 値を変えない包みのうち、**中の式が最後の子**にあるもの。
///
/// 型を先に書く言い当て（`<NodeRequire>require`。最初の子は型）と、順に評価して
/// 最後を返す並び（`(0, require)`）。
///
/// **上下どちらの走査も同じ表を引く。** 上へ辿る側だけを広げると、`specifier_of` が
/// 採れない綴りを「呼ばれる側だから無害」と答えることになり、読み込みを落とす
/// (rules/coding.md「同じ一覧を、安全な倒れ方が違う 2 箇所で使い回さない」の裏返しで、
/// **ここは 2 つの走査が同じ答えを返さないと壊れる**)。
const WRAPPERS_HOLDING_THE_VALUE_LAST: [&str; 2] = ["type_assertion", "sequence_expression"];
const IDENTIFIER_KIND: &str = "identifier";
const DYNAMIC_IMPORT_KIND: &str = "import";
const STRING_FRAGMENT_KIND: &str = "string_fragment";
/// コメント。引数の並びには現れるが、引数ではない。
const COMMENT_KIND: &str = "comment";

/// **名前を書けない**葉の種別。
///
/// 正規表現（`/\d+/`）・コメント・文字列の中のエスケープ・JSX の地の文（`C:\users`）。
/// どれも中身は綴りであって名前ではないので、**束縛も読み込みも作れない**。
///
/// 見る場所は 2 つ。[`is_escaped_name`] は「ここの逆立ちは名前ではない」として使い、
/// [`is_require_spelled_outside_a_call`] は「ここの `require` は名前ではない」として使う。
///
/// **2 箇所とも、一覧が広すぎると同じ向きへ倒れる。** 名前の書ける葉を足してしまえば、
/// 前者は名前のエスケープを、後者は本物の束縛を見逃す（どちらも偽陽性）。
/// 逆に足しそこねたときは 2 箇所とも測れない側（安全側）へ落ちるので、
/// **同じ一覧を共有してよい**（rules/coding.md「列挙で判定を組むときは、漏れの倒れる向きを選ぶ」。
/// 同じ規約が禁じているのは、**安全な倒れ方が違う** 2 箇所での使い回し）。
const KINDS_THAT_CANNOT_SPELL_A_NAME: [&str; 4] =
    [COMMENT_KIND, "regex_pattern", "escape_sequence", "jsx_text"];

/// 指定子を書ける文字列リテラルの種別。
///
/// テンプレートリテラルを入れるのは、**置換を持つものも依存の宣言だから**。
/// 種別で外すと「宣言していない」と区別が付かなくなる（読めないことは
/// [`unquoted_text_of`] が言う）。
const STRING_LITERAL_KINDS: [&str; 2] = ["string", "template_string"];

/// CommonJS が依存を読み込む関数の名前。
const REQUIRE_FUNCTION_NAME: &str = "require";

/// 型だけを運ぶ輸入・輸出に置かれる印。**名前のないノード**として木に出る。
const TYPE_ONLY_MARKER_KIND: &str = "type";

/// 印を探す先の種別。文にも個々の名前にも付けられる
/// （`import type { require }` と `import { type require }`）。
const IMPORT_EXPORT_KINDS: [&str; 4] = [
    "import_statement",
    "import_specifier",
    "export_statement",
    "export_specifier",
];

/// 型の中でしか現れない構文の種別。
///
/// **この下に書かれた名前は、実行時の値を束縛しない。** 要素の宣言
/// （`interface L { require(): void }`）だけでなく、**型の中の引数の名前**
/// （`type H = (require: string) => void`）もここへ入る。`type_query` の中の名前
/// （`typeof require`）は値を指すが、指すだけで束縛し直さない。
///
/// **値の側の構文は木の上でここへ入らない。** 実物の関数の引数は
/// `function_declaration` / `arrow_function` の下、実装を伴う定義は
/// `method_definition`、クラスの欄は `public_field_definition`、
/// `declare function` は `ambient_declaration` の下に来るので、どれも測れない側に残る。
///
/// **種別ではなく祖先で決めているのはこのため。** クラスの名前（`class require {}`）は
/// 木の上で型の名前と**同じ `type_identifier`** だが、実行時の束縛を作る。
/// 祖先が `class_declaration` でここに無いので、測れない側に残る。
///
/// **ここに無い種別は「読み込みかもしれない」側へ落ちる。** 型の中の位置を挙げ
/// そこねても、測れない側（安全側）へ落ちるだけで済む
/// (rules/coding.md「列挙で判定を組むときは、漏れの倒れる向きを選ぶ」)。
const TYPE_ONLY_KINDS: [&str; 17] = [
    "function_type",
    "constructor_type",
    "call_signature",
    "construct_signature",
    "method_signature",
    "abstract_method_signature",
    "property_signature",
    "index_signature",
    "type_query",
    "type_alias_declaration",
    "interface_declaration",
    "type_annotation",
    "type_parameter",
    "type_parameters",
    "type_arguments",
    "implements_clause",
    "function_signature",
];

/// 読み込みを起こさないと分かっている、`require` の要素の名前。
///
/// `resolve` は指定子を解決するだけで読み込まない。`cache` と `main` は値で、
/// 呼べる関数ですらない。**どれも持ち出した先で呼んでも読み込まない。**
///
/// **ここに無い名前は「読み込みかもしれない」側へ落ちる。** `require.call(null, "./dep")` /
/// `require.apply(null, ["./dep"])` は読み込みそのもので、呼ばれる側が `member_expression`
/// になるため [`specifier_of`] では採れない。落ちた先は [`RequireSpelling::Rebound`]
/// （＝読み取れない）なので、**値を作らずに測れないと言う**だけで済む
/// (rules/coding.md「列挙で判定を組むときは、漏れの倒れる向きを選ぶ」)。
const NON_LOADING_REQUIRE_MEMBERS: [&str; 3] = ["resolve", "cache", "main"];

/// 呼ばれる側を**その場で呼ぶ** `Function.prototype` の要素の名前。
///
/// **[`NON_LOADING_REQUIRE_MEMBERS`] と別の一覧にしてある。** あちらは
/// 綴りが読める `require` の要素のうち**読み込まない**ものを外す許可リストで、
/// こちらは綴りを読み取れない添字が**呼ばれている**ことを見つける拒否リスト。
/// 漏れたときに倒れる向きが逆なので使い回さない
/// （`rules/coding.md`「同じ一覧を、安全な倒れ方が違う 2 箇所で使い回さない」）。
///
/// `bind` はここに入れない。返った関数がその場で呼ばれたときだけ数える
/// （[`FUNCTION_MEMBER_THAT_BINDS`]）。
const FUNCTION_MEMBERS_THAT_INVOKE: [&str; 2] = ["call", "apply"];

/// 呼ばれる側を**その場では呼ばず**、束縛した関数を返す `Function.prototype` の要素の名前。
///
/// **返った関数がその場で呼ばれるかで扱いが分かれる。** `module[key].bind(null)("./dep")` /
/// `module[key].bind(null).call(null, "./dep")` は読み込みそのものだが、
/// `const load = module[key].bind(null);` は「呼ばずに持ち出して後で呼ぶ」形で、
/// 18 巡目に**残すと決着した族**に当たる（[`is_invoked_through_a_function_member`]）。
const FUNCTION_MEMBER_THAT_BINDS: &str = "bind";

/// 添字に書かれたときに、**選ばれる要素の名前が `require` になりえない**リテラルの種別。
///
/// 数を添字に書くと要素の名前は `"0"` のような綴りになるので、`require` を選べない。
/// 綴りを読み取れない添字（[`is_an_invoked_unreadable_key`]）より先に外しておかないと、
/// `handlers[0]()` のような普通のコードでそのファイルが測れなくなる。
///
/// **書いた時点で値が決まるリテラルだけを並べる。** `handlers[1 + 1]()` のように
/// 計算する形は入れない（定数畳み込みを持つことになる）。ここから漏れた形は
/// 測れない側（偽陰性）へ落ちる
/// (rules/coding.md「列挙で判定を組むときは、漏れの倒れる向きを選ぶ」)。
const INDEX_KINDS_THAT_CANNOT_SPELL_REQUIRE: [&str; 1] = ["number"];

/// そのファイルで `require` の綴りが何を指しているか。
///
/// `require` は予約語ではないので、綴りだけでは CommonJS の読み込みだと言えない
/// （動的 `import` はキーワードなのでこの曖昧さが無い）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequireSpelling {
    /// CommonJS の読み込み。呼び出しを依存の宣言として採ってよい。
    ModuleLoader,
    /// 読み込み以外の位置にも現れる綴り。同じファイルで束縛され直しているかもしれない。
    Rebound,
}

/// ソースに書かれた import 指定子。書かれた順に返す。
///
/// 文字列を無条件に拾わず、**依存を宣言する文が指定子として持っているものだけ**を採る。
/// `const path = "./pad";` を拾うと、依存していないファイルが依存しているように見える。
///
/// **読み取れない宣言が 1 つでもあれば `None` を返す。** 残りだけを返すと、
/// 呼び出し側は欠けた集合を揃った集合として扱う。
fn specifiers_of<'source>(
    tree: &SyntaxTree<'source>,
) -> Result<Vec<&'source str>, ImportsUnavailable> {
    let mut specifiers = Vec::new();
    let mut unreadable_require_call = None;

    // **綴りに依存しない宣言を先に見る。** 動的 `import` や `import` 文はキーワードなので、
    // 綴りが曖昧なファイルでも**読み取れないことは確かに言える**
    // （ファイルをまたぐときの優先順と揃える。`pipeline::import_overlap_of`）
    for node in tree.named_descendants() {
        match specifier_of(tree, node) {
            SpecifierReading::NotADeclaration => {}
            SpecifierReading::Specifier(specifier) => specifiers.push(specifier),
            SpecifierReading::Unreadable => {
                return Err(ImportsUnavailable::UnreadableDeclaration {
                    line: LineNumber::from_index(node.start_position().row),
                });
            }
            SpecifierReading::UnreadableRequireCall => {
                unreadable_require_call
                    .get_or_insert(LineNumber::from_index(node.start_position().row));
            }
        }
    }
    // 綴りが何を指すかはファイル全体を見ないと決まらないので、集めたあとに 1 回だけ決める。
    // **束縛され直しているかもしれないファイルは、呼び出しを見つけたかに関わらず
    // 測れないとして返す。** 見つけられなかった読み込みがあるかもしれず、
    // 「見つけた呼び出しの数」ではそれを言えない
    if require_spelling_of(tree) == RequireSpelling::Rebound {
        return Err(ImportsUnavailable::ReboundSpelling);
    }
    // **綴りが読み込みを指すと分かってから、`require` の呼び出しの読み取れなさを出す。**
    // 先に出すと、束縛され直したファイルのローカル関数の呼び出しを
    // 「読み取れなかった宣言」と呼ぶことになる
    if let Some(line) = unreadable_require_call {
        return Err(ImportsUnavailable::UnreadableDeclaration { line });
    }
    Ok(specifiers)
}

/// そのファイルで `require` の綴りが CommonJS の読み込みを指しているか。
///
/// 読み込み以外の位置にその綴りが 1 つでも現れたら [`RequireSpelling::Rebound`]。
/// **束縛の形を数え上げない。** 一覧を持つと、そこから漏れた束縛が
/// 「読み込み」の側へ落ちて**依存していない先を依存として数える**
/// (rules/coding.md「列挙で判定を組むときは、漏れの倒れる向きを選ぶ」)。
/// 数え上げないぶん、読み込みと関係ない綴り（`require` という中身の文字列など）でも
/// 集めない側へ倒れるが、そちらは落とすだけで済む。
fn require_spelling_of(tree: &SyntaxTree<'_>) -> RequireSpelling {
    let rebound = tree
        .named_descendants()
        .into_iter()
        .any(|node| is_require_spelled_outside_a_call(tree, node));

    if rebound {
        return RequireSpelling::Rebound;
    }
    RequireSpelling::ModuleLoader
}

/// そのノードが、[`specifier_of`] が採る呼び出し以外で `require` を指しうる名前か。
///
/// 綴りが `require` でも、[`is_use_that_cannot_load`] が挙げる位置なら数えない。
/// **綴りで比べられない名前は、`require` かどうかを決められないので数える。**
fn is_require_spelled_outside_a_call(tree: &SyntaxTree<'_>, node: Node<'_>) -> bool {
    if is_escaped_name(tree, node) {
        // 綴りを読めないので `require` かどうかを決められないが、位置で言えることはある
        return !is_use_that_cannot_load_whatever_it_spells(tree, node);
    }
    if is_unreadable_computed_key(tree, node) || is_an_invoked_unreadable_key(tree, node) {
        return true;
    }
    if tree.text_of(node) != Some(REQUIRE_FUNCTION_NAME) {
        return false;
    }
    if KINDS_THAT_CANNOT_SPELL_A_NAME.contains(&node.kind()) {
        return false;
    }
    if is_a_dependency_literal(tree, node) {
        return false;
    }
    !is_use_that_cannot_load(tree, node)
}

/// そのノードが、**呼ばれている**添字アクセスのうち、綴りを読み取れないものか。
///
/// `module["re" + "quire"]("./dep")` は読み込みだが、**どのノードも `require` を綴らない**
/// ので、[`is_require_spelled_outside_a_call`] の一致でも [`is_unreadable_computed_key`]
/// でも見つけられない（後者は添字が文字列リテラルのときだけを見る）。
///
/// **呼ばれているものだけを見る。** 呼ばれていない添字（`arr[i]` / `map[key]`）まで
/// 落とすと、ごく普通のコードがすべて測れなくなる。そのぶん
/// `const load = module[key]; load("./dep");` は漏れるが、漏れは
/// [`RequireSpelling::Rebound`] ではなく**取りこぼし**になる。
///
/// **`call` / `apply` / `bind` 越しも呼ばれている**（[`is_invoked_on_the_spot`]）。
///
/// **Why not（添字を解いて綴りを組み直す）**: 定数畳み込みを持つことになる。
/// `"re" + "quire"` を解けても、次は `[..."require"].join("")` が来る。
fn is_an_invoked_unreadable_key(tree: &SyntaxTree<'_>, node: Node<'_>) -> bool {
    if node.kind() != SUBSCRIPT_EXPRESSION_KIND || !is_invoked_on_the_spot(tree, node) {
        return false;
    }
    let Some(index) = node.child_by_field_name("index") else {
        return false;
    };
    let written = inside_wrappers(index);
    if INDEX_KINDS_THAT_CANNOT_SPELL_REQUIRE.contains(&written.kind()) {
        return false;
    }

    unquoted_text_of(tree, written).is_none()
}

/// そのノードが、[`specifier_of`] が指定子として読む文字列リテラルの中身か。
///
/// **依存先の名前が `require` でも、それはこのファイルの名前ではない**
/// （`import loader from "require"` / `require("require")`）。指定子として読まれる
/// 綴りまで名前として数えると、**読み取れている宣言を読み取れないと答える**。
///
/// 見るのは [`specifier_of`] が読む 3 つの位置だけ。**広げると逆向きに倒れる**
/// （指定子でない綴りを見逃す = 偽陽性）ので、位置は同じ欄・同じ順序で確かめる。
fn is_a_dependency_literal(tree: &SyntaxTree<'_>, node: Node<'_>) -> bool {
    let literal = outside_a_string_literal(node);
    let Some(parent) = literal.parent() else {
        return false;
    };

    if DEPENDENCY_STATEMENT_KINDS.contains(&parent.kind()) {
        return parent.child_by_field_name("source") == Some(literal);
    }
    if parent.kind() == IMPORT_REQUIRE_CLAUSE_KIND {
        return first_string_child_of(parent) == Some(literal);
    }
    is_the_first_argument_of_a_dependency_call(tree, literal)
}

/// その文字列リテラルが、指定子を受け取る呼び出しの**最初の引数**か。
///
/// [`reading_of_argument`] が読むのと同じ 1 つに限る。2 つ目以降は指定子として
/// 読まれないので、そこに書かれた `require` は名前として数える側に残す。
fn is_the_first_argument_of_a_dependency_call(tree: &SyntaxTree<'_>, literal: Node<'_>) -> bool {
    // 包みは値を変えないので、引数の位置は包みの外で見る（`require(("./pad"))`）
    let written = outside_wrappers(literal);
    let Some(arguments) = written.parent() else {
        return false;
    };
    let Some(call) = arguments.parent() else {
        return false;
    };
    let is_a_dependency_call = calls_dynamic_import(call) || calls_require(tree, call);

    is_a_dependency_call
        && call.child_by_field_name("arguments") == Some(arguments)
        && first_expression_child_of(arguments) == Some(written)
}

/// そのノードの直下にある最初の文字列リテラル。
fn first_string_child_of(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();

    node.named_children(&mut cursor)
        .find(|child| STRING_LITERAL_KINDS.contains(&child.kind()))
}

/// そのノードが、綴りを読み取れない欄の名前か。
///
/// **欄を読む名前は、綴って書いても添字で書いても同じ働きをする。**
/// `module["require"]("./dep")` も `const { require: load } = module` も
/// `module.require` の読み出しで、綴りが読めれば
/// [`is_require_spelled_outside_a_call`] の一致が捕まえる。読めないと
/// **`require` かどうかを決められない**。
///
/// [`KINDS_THAT_CANNOT_SPELL_A_NAME`] が `escape_sequence` を挙げているのは
/// 「文字列の中の逆立ちは名前ではない」という理由だが、**欄を読む文字列だけは名前になる**。
fn is_unreadable_computed_key(tree: &SyntaxTree<'_>, node: Node<'_>) -> bool {
    if !STRING_LITERAL_KINDS.contains(&node.kind()) {
        return false;
    }

    reads_a_field_by_this_string(node)
        && unquoted_text_of(tree, node).is_none()
        && spells_a_static_name(node)
}

/// その文字列リテラルが、**欄を読む**位置に置かれているか。
///
/// 2 つある。添字（`module["require"]`）と、分解して受ける側の欄の名前
/// （`const { ["require"]: load } = module`）。どちらも欄を読み出す。
///
/// **書く側（`pair`）は含めない。** `{ ["require"]: false }` は欄を作るだけで、
/// 読み出しにならない（`rules/coding.md`「列挙で判定を組むときは、漏れの倒れる向きを選ぶ」の
/// 適用で、ここは広げると読み取れる普通のコードまで測れなくなる側へ倒れる）。
fn reads_a_field_by_this_string(node: Node<'_>) -> bool {
    let positioned = outside_wrappers(node);
    let Some(parent) = positioned.parent() else {
        return false;
    };
    if parent.kind() == SUBSCRIPT_EXPRESSION_KIND
        && parent.child_by_field_name("index") == Some(positioned)
    {
        return true;
    }

    let written = outside_a_computed_key(positioned);
    let Some(owner) = written.parent() else {
        return false;
    };

    owner.kind() == PAIR_PATTERN_KIND && owner.child_by_field_name("key") == Some(written)
}

/// 計算された欄を包むものの外まで戻る。
fn outside_a_computed_key(node: Node<'_>) -> Node<'_> {
    node.parent()
        .filter(|parent| parent.kind() == COMPUTED_PROPERTY_NAME_KIND)
        .unwrap_or(node)
}

/// その文字列リテラルの綴りが、**書いた時点で決まっている**か。
///
/// **呼ばれているかを見る／見ないの分かれ目。** エスケープは書いた時点で綴りが決まって
/// いるので、呼ばれる前に持ち出されても `require` でないとは言えない。
/// 置換（`` `title-${locale}` ``）や空は**実行時に決まる**ので、`config[key]` と同じく
/// 呼ばれているときだけ見る（[`is_an_invoked_unreadable_key`]）。
///
/// **置換が 1 つでもあれば、エスケープが混ざっていても実行時に決まる**
/// （`` config[`titlei-${locale}`] ``）。エスケープの有無だけで決めると、
/// 呼ばれてもいない普通の動的な読み出しまで測れなくなる。
fn spells_a_static_name(literal: Node<'_>) -> bool {
    let mut cursor = literal.walk();
    let mut has_an_escape = false;

    for child in literal.named_children(&mut cursor) {
        if child.kind() == TEMPLATE_SUBSTITUTION_KIND {
            return false;
        }
        if child.kind() == ESCAPE_SEQUENCE_KIND {
            has_an_escape = true;
        }
    }
    has_an_escape
}

/// そのノードが、エスケープを含む名前か。
///
/// 名前にはエスケープを書ける（`require` は JS の上では `require` と同じ名前）が、
/// 木が返すのは**書かれた綴りのまま**なので、`require` との一致では見つけられない。
/// 見つけられないまま呼び出しを採ると、**存在しない依存**を集合に入れる。
///
/// 見るのは葉だけ。親は子の綴りを丸ごと含むので、葉に無い逆立ちは無い。
fn is_escaped_name(tree: &SyntaxTree<'_>, node: Node<'_>) -> bool {
    if node.named_child_count() != 0 {
        return false;
    }
    if KINDS_THAT_CANNOT_SPELL_A_NAME.contains(&node.kind()) {
        return false;
    }
    tree.text_of(node).is_some_and(|text| text.contains('\\'))
}

/// そのノードが、**新しい名前も読み込みも作りえない**位置に置かれた名前か。
///
/// 呼び出しの呼ばれる側（`require("./dep")`。読み込みだが、[`specifier_of`] が採る）と、
/// [`is_use_that_cannot_load_whatever_it_spells`] が挙げる位置。
///
/// **綴りが `require` と一致したときだけ使う。** 綴りを読めないときは
/// [`is_use_that_cannot_load_whatever_it_spells`] のほうを直に呼ぶ（理由はそちらの doc）。
///
/// **「呼ばれていないから inert」は成り立たない。** 呼ばれていない参照は**値として
/// 持ち出せる**ので、後から呼ばれうる（`const load = module.require.bind(module);`）。
/// 要素の名前（`module.require` / `registry.require`）を inert と言えないのはこのため
/// （`module` と `registry` を木の上で区別できない）。
///
/// **型の中で宣言された要素の名前だけは別**（[`TYPE_ONLY_KINDS`]）。値を持たないので
/// 持ち出せず、その型を使う側の `loader.require(…)` は要素アクセスとして別に数える。
///
/// **ここから漏れた位置は [`RequireSpelling::Rebound`] へ落ちる。** 落ちた先は
/// 「読み取れない」なので、**値を作らずに測れないと言う**だけで済む。
fn is_use_that_cannot_load(tree: &SyntaxTree<'_>, node: Node<'_>) -> bool {
    is_invoked(node) || is_use_that_cannot_load_whatever_it_spells(tree, node)
}

/// そのノードが、**綴りを読めなくても**読み込みを起こさないと言える位置か。
///
/// [`is_use_that_cannot_load`] から**呼ばれる側だけを外した**もの。
///
/// **同じ一覧を、安全な倒れ方が違う 2 箇所で使い回さない**
/// (rules/coding.md)。呼ばれる側を数えなくてよいのは、**綴りが読めれば
/// [`specifier_of`] が採るから**であって、その位置が無害だからではない。
/// 綴りを読めないとき（[`is_escaped_name`]）は `specifier_of` も採れないので、
/// 呼ばれる側を外すと**読み込みを 1 件落としたまま集合を返す**。
///
/// 残りの位置は綴りに依らない。型空間・欄の名前・ラベル・JSX の名前は値にならず、
/// 要素の受け側と単項演算の対象は、その名前が読み込む関数そのものでも読み込みを起こさない。
fn is_use_that_cannot_load_whatever_it_spells(tree: &SyntaxTree<'_>, node: Node<'_>) -> bool {
    // 包みは値を変えないので、外まで戻ってから位置を見る（`(require)("./dep")`）
    let positioned = outside_wrappers(node);
    let Some(parent) = positioned.parent() else {
        return false;
    };

    let reads_by_name =
        parent.kind() == MEMBER_EXPRESSION_KIND && accesses_a_non_loading_member(tree, parent);
    let reads_by_key = parent.kind() == SUBSCRIPT_EXPRESSION_KIND
        && reads_a_non_loading_member_by_key(tree, parent);
    let is_non_loading_member_object =
        parent.child_by_field_name("object") == Some(positioned) && (reads_by_name || reads_by_key);
    let is_unary_operand = parent.kind() == UNARY_EXPRESSION_KIND
        && parent.child_by_field_name("argument") == Some(positioned);
    let is_written_in_a_type = is_written_only_in_a_type(node);
    let is_renamed_on_the_way_in = is_the_imported_side_of_a_rename(node);
    let is_an_object_key = is_an_object_key(node);
    let is_an_asserted_type = is_the_type_side_of_an_assertion(node);
    let is_a_jsx_attribute_name = is_a_jsx_attribute_name(node);
    let is_a_statement_label = node.kind() == STATEMENT_IDENTIFIER_KIND;
    let is_a_jsx_element_name = is_a_jsx_element_name(node);

    is_non_loading_member_object
        || is_unary_operand
        || is_written_in_a_type
        || is_renamed_on_the_way_in
        || is_an_object_key
        || is_an_asserted_type
        || is_a_jsx_attribute_name
        || is_a_statement_label
        || is_a_jsx_element_name
}

/// そのノードが、JSX の要素の**タグの名前**か。
///
/// 綴りが `require` と一致するのは小文字のときだけで、**小文字のタグは組み込み要素**。
/// JSX は文字列としてコンパイルするので、束縛も参照も作らない
/// （大文字で始まるタグは名前を参照するが、そちらは綴りが一致しない）。
///
/// 開始・終了・自己閉じの 3 つとも `name` の欄にタグの名前を持つ。
fn is_a_jsx_element_name(node: Node<'_>) -> bool {
    let written = outside_a_jsx_namespace(node);
    let Some(parent) = written.parent() else {
        return false;
    };
    if !JSX_ELEMENT_KINDS.contains(&parent.kind()) {
        return false;
    }

    parent.child_by_field_name("name") == Some(written)
}

/// そのノードが、JSX の属性の**名前**か。
///
/// `<Widget require={false} />` の属性の名前は、オブジェクトの欄の名前と同じく
/// 束縛でも参照でもない。値を渡す側（`<Widget loader={require} />`）は
/// `jsx_expression` の下に来るのでここへは入らない。
///
/// **名前の位置に限る。** 属性の下の最初の名前だけを見て、値の側
/// （`<Widget loader={require} />` / `<Widget label="require" />`）は数えたまま残す。
///
/// **Why not（欄の名前と同じく文字列の外まで戻す）**: JSX に引用符つきの属性名は無いので、
/// 戻しても答えが変わらない。`is_an_object_key` が戻すのは `{ "require": false }` があるため。
fn is_a_jsx_attribute_name(node: Node<'_>) -> bool {
    let written = outside_a_jsx_namespace(node);
    let Some(parent) = written.parent() else {
        return false;
    };
    if parent.kind() != JSX_ATTRIBUTE_KIND {
        return false;
    }

    parent.named_child(0) == Some(written)
}

/// 名前空間つきの名前の中の綴りなら、その名前そのものまで戻る。
///
/// `<Widget require:mode="x" />` も `<Widget ns:require="x" />` も、綴りは属性の名前の
/// 一部でしかない。タグ（`<require:tag />`）も同じで、**前半・後半のどちらから来ても
/// 同じ位置へ戻る**。
///
/// JSX で名前空間つきの名前が置けるのはタグと属性の名前だけなので、
/// 戻した先が値になることはない。
fn outside_a_jsx_namespace(node: Node<'_>) -> Node<'_> {
    node.parent()
        .filter(|parent| parent.kind() == JSX_NAMESPACE_NAME_KIND)
        .unwrap_or(node)
}

/// そのノードが、型を言い当てる式の**型の側**の綴りか。
///
/// `value as require` / `value satisfies require` の型の側は型空間にしかいない。
///
/// **値の側は数える。** `require as NodeRequire` の `require` は読み込む関数そのもので、
/// 捕まえて持ち出せる。
///
/// `<require>value` はここへ来ない。型が `type_arguments` の下に置かれるので、
/// [`TYPE_ONLY_KINDS`] が先に拾う。
///
/// **言い当てる型の中を辿る。種別を挙げない。** 言い当てる型は 1 つの型名とは限らず
/// （`value as require | null` / `value satisfies require & Marker` / 条件型 /
/// リテラル型 `value as "require"`）、挙げていく形だと**型の書き方を 1 つ見つけるたびに
/// 測れないファイルが増える** (rules/coding.md「列挙で判定を組むときは、漏れの倒れる向きを選ぶ」)。
///
/// 辿った先が言い当てる式なら、**値になる子から来ていなければ型の側**
/// （[`value_child_of`] が値の子を持っている）。
///
/// **綴りの種別は見ない。** 言い当てる式の値の子の下に無い綴りは、その式の型の中にある。
/// 種別で先に絞ると、リテラル型（葉が `string_fragment`）が型の側だと言えなくなる。
fn is_the_type_side_of_an_assertion(node: Node<'_>) -> bool {
    let mut current = node;

    while let Some(parent) = current.parent() {
        if ASSERTION_KINDS.contains(&parent.kind()) {
            return value_child_of(parent) != Some(current);
        }
        current = parent;
    }
    false
}

/// そのノードが、ローカルの名前を作らない輸入・輸出の綴りか。
///
/// 2 つある。
///
/// **別名を付けた輸入の輸入元の側**（`import { require as load }`）。実行時の名前に
/// なるのは `load` だけ。**別名の側（`import { load as require }`）は数える。**
/// 別名が無い `import { require }` も数える。
///
/// **輸出元を持つ輸出（再輸出）の綴り**（`export { require as load } from "./x"`）。
/// 相手のモジュールの輸出名を並べているだけで、このファイルの束縛を 1 つも作らない。
/// **輸出元を持たない輸出（`export { require as load };`）は数える。** そちらは
/// このファイルの束縛を指す（CommonJS のファイルなら読み込む関数そのもの）。
///
/// **綴りは文字列でも書ける**（`import { "require" as load }` / `export { load as "require" }`。
/// ES2022）。木の上では `string` を 1 つ挟むだけなので、欄を見る前に外まで戻す。
/// 見る欄は変えない。**文字列で書けるのはローカルの名前にならない側だけ**なので、
/// 戻したうえで同じ欄を確かめれば、束縛を作る側を取り違えない。
fn is_the_imported_side_of_a_rename(node: Node<'_>) -> bool {
    let written = outside_a_string_literal(node);
    let Some(parent) = written.parent() else {
        return false;
    };
    match parent.kind() {
        IMPORT_SPECIFIER_KIND => {
            let is_the_name = parent.child_by_field_name("name") == Some(written);
            is_the_name && parent.child_by_field_name("alias").is_some()
        }
        EXPORT_SPECIFIER_KIND => {
            // 別名の側は輸出される名前でしかなく、輸出元の有無に関わらず束縛を作らない
            let is_the_alias = parent.child_by_field_name("alias") == Some(written);
            is_the_alias || is_inside_a_re_export(parent)
        }
        NAMESPACE_EXPORT_KIND => is_inside_a_re_export(parent),
        _ => false,
    }
}

/// その輸出の指定が、輸出元を持つ輸出（再輸出）の中にあるか。
fn is_inside_a_re_export(specifier: Node<'_>) -> bool {
    let mut ancestor = specifier.parent();

    while let Some(current) = ancestor {
        if current.kind() == EXPORT_STATEMENT_KIND {
            return current.child_by_field_name("source").is_some();
        }
        ancestor = current.parent();
    }
    false
}

/// そのノードが、オブジェクトの欄の名前か。
///
/// `const options = { require: false };` の `require` は束縛でも参照でもない。
/// あとで読むときは `options.require` の要素アクセスになるので、そちらで数えれば足りる。
///
/// **省略記法（`{ require }`）は数える。** そちらは `shorthand_property_identifier` と
/// 別の種別で、**名前を参照する**（読み込む関数そのものをオブジェクトへ持ち出せる）。
///
/// **書く側（`pair`）だけを外す。** 分解して受ける側（`pair_pattern`）は綴りが同じでも
/// **欄を読む**ので、`const { require: load } = module;` は `const load = module.require;`
/// と同じ持ち出しになる。要素アクセスの側を「読み込むかもしれない」に残している以上、
/// 分解の側だけ外すと**読み込みを 1 件落としたまま集合を返す**。
///
/// **Why not（`pair_pattern` の `key` も外す）**: 依存先が食い違う 2 ファイルが
/// `EXTRACT-CANDIDATE`・重なり `1.00` になる（実測）。束縛を作るのは `load` だけだが、
/// このモジュールが見ているのは束縛ではなく**読み込みが届きうるか**
/// (rules/architecture.md「取れなかったシグナルを既定値で埋めない」)。
fn is_an_object_key(node: Node<'_>) -> bool {
    let written = outside_a_written_key(node);
    let Some(parent) = written.parent() else {
        return false;
    };

    parent.kind() == PAIR_KIND && parent.child_by_field_name("key") == Some(written)
}

/// 綴りで書かれた欄の名前を包むものの外まで戻る。
///
/// 引用符で囲んだ欄は文字列を 1 つ（`{ "require": false }`）、計算された欄は
/// さらに 1 つ挟む（`{ ["require"]: false }`）。
///
/// **計算された欄を剥がすのは、中が文字列リテラルのときだけ。** 名前を書いた
/// `{ [require]: 1 }` は**値の参照**なので、剥がすと読み込む関数の持ち出しを見落とす。
fn outside_a_written_key(node: Node<'_>) -> Node<'_> {
    let literal = outside_a_string_literal(node);
    if literal == node {
        return node;
    }

    literal
        .parent()
        .filter(|parent| parent.kind() == COMPUTED_PROPERTY_NAME_KIND)
        .unwrap_or(literal)
}

/// 文字列リテラルの中の綴りなら、その文字列そのものまで戻る。
fn outside_a_string_literal(node: Node<'_>) -> Node<'_> {
    node.parent()
        .filter(|parent| STRING_LITERAL_KINDS.contains(&parent.kind()))
        .unwrap_or(node)
}

/// そのノードが、型空間にだけ置かれた名前か。
///
/// **祖先をすべて辿る。** 型の中の名前は、要素の宣言（親が [`TYPE_ONLY_KINDS`]）と
/// 引数の名前（`required_parameter` を挟む）で親までの深さが違うので、
/// 親だけを見ると片方が漏れる。
///
/// 型だけの輸入・輸出は種別では見分けられない（下の [`spells_a_type_only_marker`]）。
fn is_written_only_in_a_type(node: Node<'_>) -> bool {
    let mut ancestor = node.parent();

    while let Some(current) = ancestor {
        if TYPE_ONLY_KINDS.contains(&current.kind()) {
            return true;
        }
        if IMPORT_EXPORT_KINDS.contains(&current.kind()) && spells_a_type_only_marker(current) {
            return true;
        }
        ancestor = current.parent();
    }
    false
}

/// その輸入・輸出が、型だけを運ぶ印を持っているか。
///
/// **印は名前のないノード。** `import type { require }` と `import { require }` は
/// **名前のあるノードだけを見ると同じ木**になるので、名前のない子まで見ないと
/// 区別できない。綴りで探すと `import { type as require }`（`type` という名前を
/// `require` へ別名にした**値**の輸入）を取り違える。
///
/// **名前のない子だけを数えるのは、印がキーワードだから。** 同じ綴りの名前付きノードを
/// 印と取り違えると、値の輸入を型だけの輸入として外す（安全でない側）。
/// 今の文法にそういうノードは無いので**この条件を外してもテストは通る**が、
/// 外れたときの向きが安全でないほうなので残す。
fn spells_a_type_only_marker(node: Node<'_>) -> bool {
    let mut cursor = node.walk();

    node.children(&mut cursor)
        .any(|child| !child.is_named() && child.kind() == TYPE_ONLY_MARKER_KIND)
}

/// その要素アクセスが、読み込みを起こさないと分かっている名前を指しているか。
///
/// **呼ばれているかは見ない。** `require.resolve` は読んだだけでも持ち出せるが、
/// 持ち出した先で呼んでも読み込まない。名前そのものが答えを決める。
///
/// 計算された添字（`require["call"]`）は `member_expression` にならないので、
/// ここへ来ない（＝読み込みかもしれない側へ落ちる）。
fn accesses_a_non_loading_member(tree: &SyntaxTree<'_>, member: Node<'_>) -> bool {
    let Some(name) = member.child_by_field_name("property") else {
        return false;
    };

    tree.text_of(name)
        .is_some_and(|text| NON_LOADING_REQUIRE_MEMBERS.contains(&text))
}

/// その添字アクセスが、読み込みを起こしえない要素を**綴りの読める添字**で読んでいるか。
///
/// `require["resolve"]` は `require.resolve` と同じ要素で、書き方が違うだけ。
/// **読み取れない添字は対象外**（`require` の要素かどうかを決められない）。
fn reads_a_non_loading_member_by_key(tree: &SyntaxTree<'_>, subscript: Node<'_>) -> bool {
    let Some(index) = subscript.child_by_field_name("index") else {
        return false;
    };

    unquoted_text_of(tree, inside_wrappers(index))
        .is_some_and(|text| NON_LOADING_REQUIRE_MEMBERS.contains(&text))
}

/// その式が、呼び出しか `new` の呼ばれる側になっているか。
///
/// **[`is_invoked`] と分けてある。** あちらは「[`specifier_of`] が採る呼び出しの形か」を
/// 見ており、`new require("./dep")` を採らない以上そこへ `new` を足すと**依存が黙って落ちる**。
/// こちらは「読み込みが起きうるか」を見るので、`new` も数える
/// （`rules/coding.md`「同じ一覧を、安全な倒れ方が違う 2 箇所で使い回さない」）。
fn is_invoked_or_constructed(node: Node<'_>) -> bool {
    if is_invoked(node) {
        return true;
    }
    let positioned = outside_wrappers(node);
    let Some(parent) = positioned.parent() else {
        return false;
    };

    parent.kind() == NEW_EXPRESSION_KIND
        && parent.child_by_field_name("constructor") == Some(positioned)
}

/// その式の値が、**持ち出されずにその場で呼ばれる**か。
///
/// 直に呼ぶ・`new` する形（[`is_invoked_or_constructed`]）と、`Function.prototype` の
/// 要素を通す形（[`is_invoked_through_a_function_member`]）をまとめて見る。
///
/// **「呼ばれているか」を尋ねる側はここだけを見る。** 呼び方は `.call` / `.apply` /
/// `bind` と重ねられるので、片方だけを見ると重ねた形が漏れる。
fn is_invoked_on_the_spot(tree: &SyntaxTree<'_>, node: Node<'_>) -> bool {
    is_invoked_or_constructed(node) || is_invoked_through_a_function_member(tree, node)
}

/// その式が、`Function.prototype` の要素を通してその場で呼ばれているか。
///
/// `module["re" + "quire"].call(null, "./dep")` は読み込みだが、添字の親は
/// `member_expression` なので [`is_invoked`] では見つからない。
///
/// **`bind` は返った関数がその場で呼ばれるかまで見る。** `bind` 自体は値を返すだけなので、
/// `const load = module[key].bind(null); load("./dep");` は「呼ばずに持ち出して
/// 後で呼ぶ」形で、18 巡目に**残すと決着した族**に当たる。
///
/// **返った関数の呼び方も重ねられる**（`bind(null).call(…)` / `bind(null).bind(null)(…)`）。
/// [`is_invoked_on_the_spot`] を通して辿るので、重ねた形も同じ 1 つの条件で拾う。
/// 辿る先は必ず木の上へ進むので止まる。
///
/// **要素の綴りは 2 通りある**（`.call` と `["call"]`）。同じことが起きるので
/// 綴り方で分けない（`rules/naming.md`「`require` を `import` と別の語にしない」と同じ形）。
///
/// **ここから漏れた呼び方**（`Reflect.apply(module[k], …)` など）**も同じ族。**
/// 呼ぶ相手を別の関数へ渡す形は、追うのにデータフロー解析が要る。
fn is_invoked_through_a_function_member(tree: &SyntaxTree<'_>, node: Node<'_>) -> bool {
    let positioned = outside_wrappers(node);
    let Some(access) = positioned.parent() else {
        return false;
    };
    if access.child_by_field_name("object") != Some(positioned) {
        return false;
    }
    let Some(member) = function_member_read_by(tree, access) else {
        return false;
    };
    let Some(applied) = invoking_call_of(access) else {
        return false;
    };

    FUNCTION_MEMBERS_THAT_INVOKE.contains(&member)
        || (member == FUNCTION_MEMBER_THAT_BINDS && is_invoked_on_the_spot(tree, applied))
}

/// その要素アクセスが読んでいる要素の名前。読み取れなければ `None`。
///
/// 綴りで書く形（`.call`）と添字で書く形（`["call"]`）の両方を受ける。
fn function_member_read_by<'source>(
    tree: &SyntaxTree<'source>,
    access: Node<'_>,
) -> Option<&'source str> {
    match access.kind() {
        MEMBER_EXPRESSION_KIND => tree.text_of(access.child_by_field_name("property")?),
        SUBSCRIPT_EXPRESSION_KIND => {
            unquoted_text_of(tree, inside_wrappers(access.child_by_field_name("index")?))
        }
        _ => None,
    }
}

/// その式が、呼び出しの呼ばれる側になっているか。包みは外へ辿る。
fn is_invoked(node: Node<'_>) -> bool {
    invoking_call_of(node).is_some()
}

/// その式を呼んでいる呼び出し。呼ばれていなければ `None`。包みは外へ辿る。
fn invoking_call_of(node: Node<'_>) -> Option<Node<'_>> {
    let positioned = outside_wrappers(node);
    let parent = positioned.parent()?;

    (parent.kind() == CALL_EXPRESSION_KIND
        && parent.child_by_field_name("function") == Some(positioned))
    .then_some(parent)
}

/// 包んでいる包みの外まで遡ったノード。
fn outside_wrappers(node: Node<'_>) -> Node<'_> {
    let mut current = node;

    while let Some(parent) = current.parent() {
        if value_child_of(parent) != Some(current) {
            break;
        }
        current = parent;
    }
    current
}

/// その包みの中で、値になる子。包みでなければ `None`。
///
/// **上へ辿る側も下へ剥がす側も、この 1 つを引く。** 片方だけが知っている包みがあると、
/// 「呼ばれる側だから `specifier_of` が採る」という前提が崩れる。
fn value_child_of(wrapper: Node<'_>) -> Option<Node<'_>> {
    if WRAPPERS_HOLDING_THE_VALUE_FIRST.contains(&wrapper.kind()) {
        return first_expression_child_of(wrapper);
    }
    if WRAPPERS_HOLDING_THE_VALUE_LAST.contains(&wrapper.kind()) {
        return last_expression_child_of(wrapper);
    }
    None
}

/// そのノードの直下にある最後の式。コメントは式ではないので飛ばす。
fn last_expression_child_of(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();

    node.named_children(&mut cursor)
        .filter(|child| child.kind() != COMMENT_KIND)
        .last()
}

/// 包みを剥がした中の式。
fn inside_wrappers(node: Node<'_>) -> Node<'_> {
    let mut current = node;

    while let Some(inner) = value_child_of(current) {
        current = inner;
    }
    current
}

/// そのノードの直下にある最初の式。コメントは式ではないので飛ばす。
fn first_expression_child_of(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();

    node.named_children(&mut cursor)
        .find(|child| child.kind() != COMMENT_KIND)
}

/// ノード 1 つを依存の宣言として読んだ結果。
enum SpecifierReading<'source> {
    /// 依存を宣言していないノード。
    NotADeclaration,
    /// 読み取れた指定子。
    Specifier(&'source str),
    /// 依存を宣言しているが、指定子を読み取れなかった。**`require` の綴りに依存しない形**
    /// （`import` 文・`export ... from`・動的 `import`・import-equals）。
    ///
    /// **落とさずにここへ出す。** 黙って落とすと、同じファイルの他の宣言だけで
    /// 重なりが測れてしまう (rules/architecture.md「取れなかったシグナルを既定値で埋めない」)。
    Unreadable,
    /// `require` の呼び出しに見えるが、指定子を読み取れなかった。
    ///
    /// **[`Unreadable`] と分けてある。** 綴りが束縛され直しているファイルでは、この呼び出しは
    /// **依存の宣言ですらない**（利用者のローカル関数）。まとめて「読み取れなかった宣言がある」と
    /// 答えると、**確かめていないことを確かめた答えとして出す**ことになり、`--explain` が
    /// 利用者を dryguard の穴へ向かせる (rules/architecture.md「理由は落とさない」)。
    ///
    /// [`Unreadable`]: SpecifierReading::Unreadable
    UnreadableRequireCall,
}

/// そのノードが宣言している依存先の指定子。
///
/// **綴りが束縛され直しているファイルもここへ来る。** 何が束縛され直しているかは
/// ファイル全体を見ないと決まらないので、[`specifiers_of`] が集めたあとに 1 回だけ決める。
/// `require` の呼び出しから読み取れなかったことは、そのぶん
/// [`SpecifierReading::UnreadableRequireCall`] として別に返す。
fn specifier_of<'source>(tree: &SyntaxTree<'source>, node: Node<'_>) -> SpecifierReading<'source> {
    if DEPENDENCY_STATEMENT_KINDS.contains(&node.kind()) {
        let Some(source) = node.child_by_field_name("source") else {
            return SpecifierReading::NotADeclaration;
        };
        return reading_of(tree, source);
    }
    if node.kind() == IMPORT_REQUIRE_CLAUSE_KIND {
        return reading_of_first_string_child(tree, node);
    }
    if calls_dynamic_import(node) {
        return reading_of_argument(tree, node);
    }
    if calls_require(tree, node) {
        return match reading_of_argument(tree, node) {
            SpecifierReading::Unreadable => SpecifierReading::UnreadableRequireCall,
            reading => reading,
        };
    }
    SpecifierReading::NotADeclaration
}

/// その呼び出しが動的 import か（`import("./pad")`）。
fn calls_dynamic_import(node: Node<'_>) -> bool {
    if node.kind() != CALL_EXPRESSION_KIND {
        return false;
    }

    node.child_by_field_name("function")
        .is_some_and(|called| called.kind() == DYNAMIC_IMPORT_KIND)
}

/// その呼び出しが `require` の綴りを呼んでいるか。
///
/// `registry.require("./pad")` は呼ばれる側が `member_expression` になるので外れる。
/// 括弧は剥がす（`(require)("./pad")` も同じ呼び出し）。
fn calls_require(tree: &SyntaxTree<'_>, node: Node<'_>) -> bool {
    if node.kind() != CALL_EXPRESSION_KIND {
        return false;
    }

    node.child_by_field_name("function").is_some_and(|called| {
        let called = inside_wrappers(called);
        called.kind() == IDENTIFIER_KIND && tree.text_of(called) == Some(REQUIRE_FUNCTION_NAME)
    })
}

/// その呼び出しの**最初の引数**から読み取った指定子。
///
/// **2 つ目以降は見ない。** `require(name, "./fallback")` の `"./fallback"` は
/// 読み込む先ではないので、拾うと**どちらのファイルも読み込んでいない依存**が
/// 集合に入る。最初の引数が文字列リテラルでなければ読み取れないこととして返す
/// （`require(name)` も依存の宣言ではある）。
fn reading_of_argument<'source>(
    tree: &SyntaxTree<'source>,
    call: Node<'_>,
) -> SpecifierReading<'source> {
    let Some(arguments) = call.child_by_field_name("arguments") else {
        return SpecifierReading::Unreadable;
    };
    let Some(written) = first_expression_child_of(arguments) else {
        return SpecifierReading::Unreadable;
    };
    // 包みは値を変えないので、綴りを見る前に剥がす（`require(("./pad"))`）
    let first = inside_wrappers(written);
    if !STRING_LITERAL_KINDS.contains(&first.kind()) {
        return SpecifierReading::Unreadable;
    }

    reading_of(tree, first)
}

/// そのノードの直下にある最初の文字列リテラルから読み取った指定子。
fn reading_of_first_string_child<'source>(
    tree: &SyntaxTree<'source>,
    node: Node<'_>,
) -> SpecifierReading<'source> {
    let Some(literal) = first_string_child_of(node) else {
        return SpecifierReading::Unreadable;
    };
    reading_of(tree, literal)
}

/// その文字列リテラルから読み取った指定子。
fn reading_of<'source>(tree: &SyntaxTree<'source>, literal: Node<'_>) -> SpecifierReading<'source> {
    let Some(text) = unquoted_text_of(tree, literal) else {
        return SpecifierReading::Unreadable;
    };

    SpecifierReading::Specifier(text)
}

/// 引用符の中身。**綴りをそのまま取り出せないときは `None`。**
///
/// 取り出せるのは、中身が**断片 1 つだけ**でできているときに限る。
/// 断片が分かれるのは次の 3 つで、どれも最初の断片を採ると別の依存先になる。
///
/// - エスケープを含む（`"./pad"` の断片は `./pa` で、続きが別のノードになる）
/// - 置換を持つテンプレートリテラル（`` import(`./${name}`) ``。書いた時点で先が決まらない）
/// - 中身が空（`import("")`。指定子として使えない）
///
/// **置換を持たないテンプレートリテラル（`` require(`./pad`) ``）は断片 1 つなので取り出せる。**
/// 引用符の違いだけで落とすと、同じ依存先を書き方の違いで別物と数える。
fn unquoted_text_of<'source>(
    tree: &SyntaxTree<'source>,
    literal: Node<'_>,
) -> Option<&'source str> {
    let mut cursor = literal.walk();
    let parts: Vec<Node<'_>> = literal.named_children(&mut cursor).collect();

    let [only] = parts.as_slice() else {
        return None;
    };
    if only.kind() != STRING_FRAGMENT_KIND {
        return None;
    }
    tree.text_of(*only)
}

/// 区切りを `/` に揃えた綴り。逆立ちを含まなければ借りたまま返す。
///
/// CommonJS は Windows の区切りで書いた相対指定（`require(".\\stock")`）も
/// importer からの相対として読み込む。揃えずに [`is_relative`] へ渡すと
/// **パッケージ名として書かれたまま残り、別々のディレクトリの同じ綴りが
/// 1 つの依存先になる**（`rules/naming.md`「`specifier` と `module path` を混ぜない」）。
///
/// **効かせるのは相対かどうかの判定と、畳む側だけ。** パッケージ名の綴りに逆立ちが
/// あっても、それは importer の位置に依らないので直さない。
fn with_forward_separators(specifier: &str) -> Cow<'_, str> {
    if !specifier.contains('\\') {
        return Cow::Borrowed(specifier);
    }

    Cow::Owned(specifier.replace('\\', "/"))
}

/// その指定子が importer の位置から解決するものか。
///
/// 区切りを伴わない `.` と `..` も相対指定。`.` を漏らすと、別々のディレクトリの
/// 入口が畳まれずに 1 つの依存先（`.`）になり、**依存していない先を共有している**ことになる。
fn is_relative(specifier: &str) -> bool {
    let starts_with_a_step = specifier.starts_with("./") || specifier.starts_with("../");
    let is_a_bare_step = specifier == "." || specifier == "..";
    starts_with_a_step || is_a_bare_step
}

/// `directory` から見た `specifier` を、`.` と `..` を畳んだ 1 本のパスにする。
fn folded_path(directory: &Path, specifier: &str) -> String {
    let mut segments: Vec<String> = directory
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => name.to_str().map(str::to_string),
            _ => None,
        })
        .collect();

    for part in specifier.split('/') {
        match part {
            "" | "." => {}
            ".." => climb(&mut segments),
            name => segments.push(name.to_string()),
        }
    }

    segments.join("/")
}

/// 1 つ上のディレクトリへ移る。
///
/// 遡る先が無いときは `..` を残す。畳めなかったことを黙って捨てると、
/// ツリーの外を指す別々の依存先が同じ値になる。
fn climb(segments: &mut Vec<String>) {
    let can_pop = segments.last().is_some_and(|segment| segment != "..");
    if can_pop {
        segments.pop();
        return;
    }
    segments.push("..".to_string());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::tree::Grammar;
    use std::path::Path;

    fn resolved(specifier: &str, importer: &str) -> String {
        ModulePath::from_specifier(specifier, Path::new(importer))
            .as_str()
            .to_string()
    }

    #[test]
    fn test_module_path_of_a_sibling_specifier_is_relative_to_the_importer() {
        assert_eq!(
            resolved("./pad", "src/utils/formatDate.ts"),
            "src/utils/pad"
        );
    }

    #[test]
    fn test_module_path_of_a_parent_specifier_climbs_out_of_the_importer_directory() {
        assert_eq!(
            resolved("../utils/pad", "src/report/dateHelper.ts"),
            "src/utils/pad"
        );
    }

    #[test]
    fn test_module_paths_of_two_importers_pointing_at_the_same_file_are_equal() {
        // このツールの最悪の外し方（共有ユーティリティに「共通化するな」と言う）は、
        // 指定子を文字列のまま比べたときに起きる
        assert_eq!(
            resolved("./pad", "src/utils/formatDate.ts"),
            resolved("../utils/pad", "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_module_path_that_climbs_past_the_top_keeps_the_remaining_parent_steps() {
        // 遡り切れなかった `..` を捨てると、ツリーの外を指す別々の依存先が同じ値になる
        assert_eq!(resolved("../../shared", "src/pad.ts"), "../shared");
    }

    #[test]
    fn test_module_path_of_a_package_specifier_is_kept_as_written() {
        // パッケージ名は importer の位置に依らない
        assert_eq!(resolved("react", "src/utils/formatDate.ts"), "react");
    }

    #[test]
    fn test_module_path_of_a_bare_dot_specifier_is_the_importers_directory() {
        // `.` は `./` と同じくそのディレクトリの入口を指す。畳まずに `.` のまま持つと、
        // 別々のディレクトリの入口が 1 つの依存先になる
        assert_eq!(resolved(".", "src/billing/a.ts"), "src/billing");
    }

    #[test]
    fn test_module_path_of_a_windows_relative_specifier_is_folded_from_the_importer() {
        // CommonJS は Windows の区切りで書いた相対指定も importer からの相対として扱う。
        // 畳まずに書かれたまま持つと、**別々のディレクトリ**の `.\\stock` が 1 つの依存先になる
        assert_eq!(
            resolved(".\\stock", "src/billing/a.ts"),
            "src/billing/stock"
        );
        assert_eq!(
            resolved("..\\shared\\pad", "src/billing/a.ts"),
            "src/shared/pad"
        );
    }

    #[test]
    fn test_module_path_of_a_package_name_containing_a_backslash_is_written_as_it_is() {
        // 対照。相対指定でない綴りはパッケージ名で、importer の位置に依らない。
        // 区切りを直して畳むと、**書かれていない依存先**になる
        assert_eq!(resolved("vendor\\pad", "src/billing/a.ts"), "vendor\\pad");
    }

    fn tree_of(source: &str) -> SyntaxTree<'_> {
        SyntaxTree::from_source(source, Grammar::TypeScript)
            .expect("テストが渡すソースは木にできる")
    }

    fn tsx_tree_of(source: &str) -> SyntaxTree<'_> {
        SyntaxTree::from_source(source, Grammar::Tsx).expect("テストが渡すソースは木にできる")
    }

    fn import_set(source: &str, importer: &str) -> ImportSet {
        ImportSet::from_tree(&tree_of(source), Path::new(importer))
            .expect("テストが渡すソースには import がある")
    }

    fn overlap(source_a: &str, importer_a: &str, source_b: &str, importer_b: &str) -> f64 {
        import_set(source_a, importer_a)
            .jaccard(&import_set(source_b, importer_b))
            .value()
    }

    const IMPORTS_PAD_FROM_SIBLING: &str = r#"import { pad } from "./pad";

export function formatDate(value: Date): string {
  return pad(value.getMonth() + 1);
}
"#;

    const IMPORTS_PAD_FROM_PARENT: &str = r#"import { pad } from "../utils/pad";

export function dateHelper(value: Date): string {
  return pad(value.getDate());
}
"#;

    #[test]
    fn test_import_overlap_of_two_files_reaching_the_same_module_is_total() {
        // 綴りが違うだけで同じ依存先。文字列のまま比べると 0.0 になる
        assert_eq!(
            overlap(
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/formatDate.ts",
                IMPORTS_PAD_FROM_PARENT,
                "src/report/dateHelper.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_overlap_of_files_depending_on_different_modules_is_zero() {
        let billing = r#"import { Invoice } from "./invoice";"#;
        let inventory = r#"import { Stock } from "./stock";"#;

        assert_eq!(
            overlap(billing, "src/billing/a.ts", inventory, "src/billing/b.ts"),
            0.0
        );
    }

    #[test]
    fn test_import_set_ignores_strings_that_are_not_import_specifiers() {
        // 拾ってよい import を 1 件、拾ってはいけない文字列を 1 件、同じソースに置く。
        // 文字列を無条件に拾う実装だと集合に "./stock" が入り、重なりが 0.5 に落ちる
        let real_import_and_a_decoy = r#"import { pad } from "./pad";
const path = "./stock";
"#;

        assert_eq!(
            overlap(
                real_import_and_a_decoy,
                "src/utils/a.ts",
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/b.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_set_of_a_multiline_import_reaches_the_same_module() {
        let split_over_lines = r#"import {
  pad,
} from "./pad";
"#;

        assert_eq!(
            overlap(
                split_over_lines,
                "src/utils/a.ts",
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/b.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_set_of_a_re_export_reaches_the_same_module() {
        let re_export = r#"export { pad } from "./pad";"#;

        assert_eq!(
            overlap(
                re_export,
                "src/utils/a.ts",
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/b.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_set_of_a_side_effect_import_reaches_the_same_module() {
        let side_effect_only = r#"import "./pad";"#;

        assert_eq!(
            overlap(
                side_effect_only,
                "src/utils/a.ts",
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/b.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_set_of_a_dynamic_import_reaches_the_same_module() {
        // import(".../pad") も依存の宣言。import 文だけを見ると、遅延読み込みしている
        // ファイルの依存先が丸ごと消える
        let dynamic = r#"export async function load(): Promise<unknown> {
  return import("./pad");
}
"#;

        assert_eq!(
            overlap(
                dynamic,
                "src/utils/a.ts",
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/b.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_set_of_a_require_call_reaches_the_same_module() {
        // require で書いた依存も同じ依存先へ届く。集めないと、この 2 つは
        // 「依存の宣言が無い」と「1 件ある」になり重なりを測れない
        let requires_pad = r#"const { pad } = require("./pad");

export function formatDate(value: Date): string {
  return pad(value.getMonth() + 1);
}
"#;

        assert_eq!(
            overlap(
                requires_pad,
                "src/utils/formatDate.ts",
                IMPORTS_PAD_FROM_PARENT,
                "src/report/dateHelper.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_set_of_an_import_equals_require_reaches_the_same_module() {
        // TypeScript の import-equals 形式。`call_expression` にならないので、
        // 呼び出しの規則だけでは拾えない
        let import_equals = r#"import pad = require("./pad");"#;

        assert_eq!(
            overlap(
                import_equals,
                "src/utils/a.ts",
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/b.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_set_of_a_require_written_as_a_statement_reaches_the_same_module() {
        // ESM 側は副作用だけの `import "./pad";` を拾う。CJS でそれに当たるのが
        // この形なので、右辺に限ると同じ形が片方でだけ拾えなくなる
        let side_effect_only = r#"require("./pad");"#;

        assert_eq!(
            overlap(
                side_effect_only,
                "src/utils/a.ts",
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/b.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_set_of_a_require_behind_a_member_access_reaches_the_same_module() {
        let behind_member_access = r#"const pad = require("./pad").pad;"#;

        assert_eq!(
            overlap(
                behind_member_access,
                "src/utils/a.ts",
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/b.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_set_of_a_file_calling_require_on_an_object_cannot_be_created() {
        // 対照に読み取れる import を 1 件置く。`module.require("./dep")` は読み込みで
        // `registry.require("./dep")` はそうでないが、**木の上では同じ形**。
        // 要素の名前というだけで外す実装だと、読み込みが落ちたままファイルが
        // 測れる側に残る
        let real_import_and_a_call_on_an_object = r#"import { pad } from "./pad";
const stock = registry.require("./stock");
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(real_import_and_a_call_on_an_object),
                Path::new("src/utils/a.ts"),
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_loading_through_module_require_cannot_be_created() {
        // Node の `module.require` は読み込みそのもの。呼ばれる側が
        // `member_expression` なので依存の宣言としては採れない
        let real_import_and_a_module_require = r#"import { pad } from "./pad";
const stock = module.require("./stock");
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(real_import_and_a_module_require),
                Path::new("src/utils/a.ts"),
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_reading_require_properties_reaches_the_same_module() {
        // `require.cache` / `require.main` は呼ばれていないので値を読むだけ。
        // 読み込まない名前の一覧だけで決める実装だと、この 2 つが束縛の側へ落ちて
        // 書いてある import まで測れなくなる
        let reads_properties = r#"const cached = require.cache;
const entry = require.main;
const pad = require("./pad");
"#;

        assert_eq!(
            overlap(
                reads_properties,
                "src/utils/a.ts",
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/b.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_set_of_a_file_referencing_an_uncalled_require_member_cannot_be_created() {
        // 対照に読み取れる import を 1 件置く。**呼ばれていない参照は値として
        // 持ち出せる**ので、後から呼ばれうる。呼ばれていないというだけで
        // 読み込まないと決める実装だと、持ち出された読み込みが落ちたまま測れる
        let real_import_and_an_uncalled_member = r#"import { pad } from "./pad";
const handler = registry.require;
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(real_import_and_an_uncalled_member),
                Path::new("src/utils/a.ts"),
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_aliasing_module_require_cannot_be_created() {
        // 対照に読み取れる import を 1 件置く。`module.require` は `.bind` の受け側に
        // なるので呼ばれる側ではないが、**別名として持ち出されて後から呼ばれる**
        let real_import_and_an_aliased_loader = r#"import { pad } from "./pad";
const load = module.require.bind(module);
load("./stock");
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(real_import_and_an_aliased_loader),
                Path::new("src/utils/a.ts"),
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_require_given_a_template_literal_reaches_the_same_module() {
        // 置換が無いので綴りは書いた時点で決まっている。引用符の違いだけで落とすと、
        // 同じ依存先を書き方の違いで別物と数える
        let templated_require = r#"const pad = require(`./pad`);"#;

        assert_eq!(
            overlap(
                templated_require,
                "src/utils/a.ts",
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/b.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_set_of_a_file_with_a_substituted_require_cannot_be_created() {
        // 対照に読み取れる import を 1 件置く。読み取れない宣言を黙って落とす実装だと
        // "./pad" だけの集合が作れてしまい、欠けた集合で重なりを測ることになる
        let real_import_and_a_substituted_require = r#"import { pad } from "./pad";
const stock = require(`./${target}`);
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(real_import_and_a_substituted_require),
                Path::new("src/utils/a.ts"),
            ),
            Err(ImportsUnavailable::UnreadableDeclaration {
                line: LineNumber::from_index(1)
            })
        );
    }

    #[test]
    fn test_import_set_of_a_file_with_an_escaped_specifier_cannot_be_created() {
        // エスケープを含む綴りは断片が分かれる。最初の断片だけを採る実装だと
        // "./pa" を依存先として集合に入れてしまう（"./pad" とは別物になる）
        let escaped = format!(
            "import {{ pad }} from \"./pa{}u0064\";\nimport {{ trim }} from \"./trim\";\n",
            char::from(92u8)
        );

        assert_eq!(
            ImportSet::from_tree(&tree_of(&escaped), Path::new("src/utils/a.ts")),
            Err(ImportsUnavailable::UnreadableDeclaration {
                line: LineNumber::from_index(0)
            })
        );
    }

    #[test]
    fn test_import_set_of_a_file_that_rebinds_require_cannot_be_created() {
        // 対照に読み取れる import を 1 件置く。`require` は予約語ではないので、
        // 綴りだけを見る実装だと集合に "./stock" が入り、重なりが 0.5 に落ちる
        let real_import_and_a_rebound_require = r#"import { pad } from "./pad";

function require(target: string): string {
  return target;
}

const stock = require("./stock");
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(real_import_and_a_rebound_require),
                Path::new("src/utils/a.ts"),
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_require_called_through_parentheses_reaches_the_same_module() {
        // 括弧は式を変えない。呼ばれる側を綴りで見る前に剥がさないと、この呼び出しは
        // 宣言として分類されないまま中の `require` が束縛と見なされる
        let through_parentheses = r#"const pad = (require)("./pad");"#;

        assert_eq!(
            overlap(
                through_parentheses,
                "src/utils/a.ts",
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/b.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_set_of_a_require_behind_a_type_annotation_reaches_the_same_module() {
        // 型だけの注記は実行時の値を変えない。剥がさないと呼び出しが宣言として
        // 分類されず、中の `require` だけが束縛と見なされる
        let annotated = r#"const pad = (require as NodeRequire)("./pad");"#;

        assert_eq!(
            overlap(
                annotated,
                "src/utils/a.ts",
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/b.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_set_of_a_require_behind_a_non_null_assertion_reaches_the_same_module() {
        let asserted = r#"const pad = require!("./pad");"#;

        assert_eq!(
            overlap(
                asserted,
                "src/utils/a.ts",
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/b.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_set_of_a_file_loading_through_require_call_cannot_be_created() {
        // 対照に読み取れる import を 1 件置く。`require.call` は束縛を作らないが
        // **読み込みそのもの**で、呼ばれる側が `member_expression` なので採れない。
        // メンバの受け側を名前を見ずに外す実装だと、この読み込みが落ちたまま
        // ファイルが測れる側に残り、重なりが 1.00 に出る
        let real_import_and_an_indirect_load = r#"import { pad } from "./pad";
const stock = require.call(null, "./stock");
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(real_import_and_an_indirect_load),
                Path::new("src/utils/a.ts"),
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_binding_require_itself_says_the_spelling_is_rebound() {
        // 宣言はすべて読み取れている。測れないのは**綴りが読み込みを指すと
        // 言い切れない**からで、dryguard が読めなかったわけではない。
        // 1 つにまとめると、利用者を dryguard の穴のほうへ向けてしまう
        let real_import_and_a_local_require = r#"import { pad } from "./pad";
function require(name: string): number {
  return name.length;
}
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(real_import_and_a_local_require),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_with_an_unreadable_specifier_says_the_declaration_is_unreadable() {
        // 対照。こちらは**書いてあるのに読めていない**ので dryguard 側の穴。
        // 綴りの曖昧さと同じ理由にすると、直す先が分からなくなる
        let unreadable_specifier = r#"import { pad } from "./pad";
const loaded = require(`./${name}`);
"#;

        assert_eq!(
            ImportSet::from_tree(&tree_of(unreadable_specifier), Path::new("src/utils/a.ts")),
            Err(ImportsUnavailable::UnreadableDeclaration {
                line: LineNumber::from_index(1)
            })
        );
    }

    #[test]
    fn test_import_set_of_a_file_asserting_a_union_containing_require_reaches_the_same_module() {
        // 言い当てる型は 1 つの型名とは限らない。組み合わせの種別を挙げていくと、
        // 型の書き方を 1 つ見つけるたびに測れないファイルが増える
        let require_in_an_asserted_union = r#"import { pad } from "./pad";
const handle = value as require | null;
"#;

        assert_eq!(
            import_set(require_in_an_asserted_union, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_asserting_an_intersection_containing_require_reaches_the_same_module()
     {
        // `satisfies` 側も同じ
        let require_in_an_asserted_intersection = r#"import { pad } from "./pad";
const handle = value satisfies require & Marker;
"#;

        assert_eq!(
            import_set(
                require_in_an_asserted_intersection,
                "src/utils/formatDate.ts"
            ),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_asserting_a_conditional_type_on_require_reaches_the_same_module() {
        // 条件型も言い当てる型の中。種別を挙げる形では届かない位置
        let require_in_an_asserted_conditional = r#"import { pad } from "./pad";
const handle = value as (require extends Loader ? string : number);
"#;

        assert_eq!(
            import_set(
                require_in_an_asserted_conditional,
                "src/utils/formatDate.ts"
            ),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_with_an_unreadable_declaration_says_which_line() {
        // 「どこかのファイルの宣言を読み取れなかった」だけでは、**直す先も、
        // dryguard の穴を再現する手がかりも渡せない**。読み取れなかった宣言は
        // 複数ある宣言のうちの 1 つなので、行が要る
        let unreadable_on_the_fourth_line = r#"import { pad } from "./pad";
import { clock } from "./clock";

const loaded = require(`./${name}`);
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(unreadable_on_the_fourth_line),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::UnreadableDeclaration {
                line: LineNumber::from_index(3)
            })
        );
    }

    #[test]
    fn test_import_set_of_a_file_with_both_causes_says_the_declaration_is_unreadable() {
        // 綴りが曖昧でも、**動的 import は綴りに依存しない宣言**。読み取れないなら
        // それは dryguard の穴で、綴りの曖昧さより先に出す（ファイルをまたぐときの
        // 優先順と揃える）
        let both_causes = r#"import { pad } from "./pad";
function require(name: string): number {
  return name.length;
}
const loaded = import(`./${name}`);
"#;

        assert_eq!(
            ImportSet::from_tree(&tree_of(both_causes), Path::new("src/utils/a.ts")),
            Err(ImportsUnavailable::UnreadableDeclaration {
                line: LineNumber::from_index(4)
            })
        );
    }

    #[test]
    fn test_import_set_of_a_file_asserting_a_literal_type_named_require_reaches_the_same_module() {
        // 言い当てる型はリテラル型でも型空間。木の上では `literal_type` を 1 段挟むだけ
        let require_as_a_literal_type = r#"import { pad } from "./pad";
const handle = value as "require";
"#;

        assert_eq!(
            import_set(require_as_a_literal_type, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_asserting_a_union_of_literal_types_reaches_the_same_module() {
        // 組み合わせの中でも同じ
        let require_in_a_literal_union = r#"import { pad } from "./pad";
const handle = value satisfies "require" | "fs";
"#;

        assert_eq!(
            import_set(require_in_a_literal_union, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_asserting_a_type_onto_a_require_string_cannot_be_created() {
        // 対照。**値の側**の文字列は素の文字列リテラルで、28 巡目の判断どおり名前として数える
        let require_string_on_the_value_side = r#"import { pad } from "./pad";
const handle = "require" as string;
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(require_string_on_the_value_side),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_instantiating_an_unreadable_computed_key_cannot_be_created() {
        // 型引数だけを与える式は実行時に消えるが、包みとして数えないと
        // 呼ばれている添字が「呼ばれていない」に見える
        let instantiated_key = r#"import { pad } from "./pad";
const load = (module["re" + "quire"]<unknown>)("./stock");
"#;

        assert_eq!(
            ImportSet::from_tree(&tree_of(instantiated_key), Path::new("src/utils/a.ts")),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_an_instantiated_require_reaches_the_same_module() {
        // 呼ばれる側が型引数を伴っても、値は `require` そのもの
        let instantiated_require = r#"const load = (require<unknown>)("../utils/pad");
"#;

        assert_eq!(
            import_set(instantiated_require, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_instantiating_a_readable_computed_key_reaches_the_same_module() {
        // 対照。読み取れる添字は型引数が付いても落とさない
        let readable_key = r#"import { pad } from "./pad";
const title = (config["title"]<string>)();
"#;

        assert_eq!(
            import_set(readable_key, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_calling_an_unreadable_computed_key_cannot_be_created() {
        // `call` は呼ばれる側を**その場で呼ぶ**。直に呼ぶのと同じ読み込みが起きる
        let called_through_call = r#"import { pad } from "./pad";
module["re" + "quire"].call(null, "./stock");
"#;

        assert_eq!(
            ImportSet::from_tree(&tree_of(called_through_call), Path::new("src/utils/a.ts")),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_applying_an_unreadable_computed_key_cannot_be_created() {
        let called_through_apply = r#"import { pad } from "./pad";
module["re" + "quire"].apply(null, ["./stock"]);
"#;

        assert_eq!(
            ImportSet::from_tree(&tree_of(called_through_apply), Path::new("src/utils/a.ts")),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_reaching_call_by_an_index_cannot_be_created() {
        // 同じ `call` を添字で書いた形。綴り方が違うだけで起きることは同じ
        let call_by_index = r#"import { pad } from "./pad";
module["re" + "quire"]["call"](null, "./stock");
"#;

        assert_eq!(
            ImportSet::from_tree(&tree_of(call_by_index), Path::new("src/utils/a.ts")),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_binding_an_unreadable_computed_key_reaches_the_same_module() {
        // 対照。`bind` はその場で呼ばず、値を持ち出すだけ。18 巡目に
        // 「呼ばずに持ち出す形は残す」と決着した族に当たる
        let bound_key = r#"import { pad } from "./pad";
const load = module["re" + "quire"].bind(null);
"#;

        assert_eq!(
            import_set(bound_key, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_calling_a_method_on_a_computed_key_reaches_the_same_module() {
        // 対照。呼ばれているのは添字が返した値の**要素**であって、値そのものではない
        let method_on_key = r#"import { pad } from "./pad";
const shown = items[index].map((row) => row);
"#;

        assert_eq!(
            import_set(method_on_key, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_invoking_a_bound_unreadable_key_cannot_be_created() {
        // `bind` が返した関数をその場で呼べば、直に呼ぶのと同じ読み込みが起きる
        let bound_then_invoked = r#"import { pad } from "./pad";
module["re" + "quire"].bind(null)("./stock");
"#;

        assert_eq!(
            ImportSet::from_tree(&tree_of(bound_then_invoked), Path::new("src/utils/a.ts")),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_constructing_a_bound_unreadable_key_cannot_be_created() {
        let bound_then_constructed = r#"import { pad } from "./pad";
const made = new (module["re" + "quire"].bind(null))("./stock");
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(bound_then_constructed),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_require_written_in_a_method_body_reaches_the_same_module() {
        // 対照。名前だけを外し、中身は今までどおり走査する
        let require_in_a_body = r#"const handlers = {
  load() {
    return require("../utils/pad");
  },
};
"#;

        assert_eq!(
            import_set(require_in_a_body, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_invoking_a_numeric_index_reaches_the_same_module() {
        // 数の添字は要素の名前が `"0"` になるので、**`require` を選べない**
        let numeric_index = r#"import { pad } from "./pad";
handlers[0]();
"#;

        assert_eq!(
            import_set(numeric_index, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_invoking_a_computed_numeric_index_cannot_be_created() {
        // 対照。計算された添字は書いた時点で綴りが決まらないので、測れない側のまま
        let computed_index = r#"import { pad } from "./pad";
handlers[1 + 1]();
"#;

        assert_eq!(
            ImportSet::from_tree(&tree_of(computed_index), Path::new("src/utils/a.ts")),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_calling_a_bound_unreadable_key_cannot_be_created() {
        // `bind` が返した関数は `call` 越しにも呼べる。呼ばれ方が 1 段増えただけ
        let bound_then_called = r#"import { pad } from "./pad";
module["re" + "quire"].bind(null).call(null, "./stock");
"#;

        assert_eq!(
            ImportSet::from_tree(&tree_of(bound_then_called), Path::new("src/utils/a.ts")),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_applying_a_bound_unreadable_key_cannot_be_created() {
        let bound_then_applied = r#"import { pad } from "./pad";
module["re" + "quire"].bind(null).apply(null, ["./stock"]);
"#;

        assert_eq!(
            ImportSet::from_tree(&tree_of(bound_then_applied), Path::new("src/utils/a.ts")),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_rebinding_a_bound_unreadable_key_cannot_be_created() {
        // `bind` は重ねられる。最後にその場で呼ばれていれば読み込みは起きる
        let bound_twice = r#"import { pad } from "./pad";
module["re" + "quire"].bind(null).bind(null)("./stock");
"#;

        assert_eq!(
            ImportSet::from_tree(&tree_of(bound_twice), Path::new("src/utils/a.ts")),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_retaining_call_on_a_bound_key_reaches_the_same_module() {
        // 対照。`call` を持ち出しただけで呼んでいないので、18 巡目の決着の族に留まる
        let call_retained = r#"import { pad } from "./pad";
const invoke = module["re" + "quire"].bind(null).call;
"#;

        assert_eq!(
            import_set(call_retained, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_shadowed_require_given_a_name_blames_the_spelling() {
        // 綴りが束縛され直しているなら、その呼び出しは依存の宣言ですらない。
        // 「読み取れなかった宣言がある」と答えると、利用者を dryguard の穴へ向かせる
        let shadowed_and_dynamic = r#"import { pad } from "./pad";
function require(path: string): string {
  return path;
}
const dep = require(name);
"#;

        assert_eq!(
            ImportSet::from_tree(&tree_of(shadowed_and_dynamic), Path::new("src/utils/a.ts")),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_shadowed_require_given_a_template_blames_the_spelling() {
        let shadowed_and_templated = r#"import { pad } from "./pad";
function require(path: string): string {
  return path;
}
const dep = require(`./${name}`);
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(shadowed_and_templated),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_shadowed_file_with_an_unreadable_import_blames_the_line() {
        // 対照。動的 `import` はキーワードで、`require` の綴りに依存しない。
        // 綴りが曖昧でも**読み取れない宣言があることは確かに言える**
        let shadowed_and_keyworded = r#"import { pad } from "./pad";
function require(path: string): string {
  return path;
}
const dep = import(`./${name}`);
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(shadowed_and_keyworded),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::UnreadableDeclaration {
                line: LineNumber::from_index(4)
            })
        );
    }

    #[test]
    fn test_import_set_of_an_unshadowed_require_given_a_name_blames_the_line() {
        // 対照。束縛し直していなければ、読み取れない指定子は本当に宣言のもの
        let unshadowed_and_dynamic = r#"import { pad } from "./pad";
const dep = require(name);
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(unshadowed_and_dynamic),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::UnreadableDeclaration {
                line: LineNumber::from_index(1)
            })
        );
    }

    #[test]
    fn test_import_set_of_a_file_awaiting_an_escaped_computed_key_cannot_be_created() {
        // `await` は Promise を解くが、**綴りとして書かれている値はそのまま**。
        // 包みとして数えないと、呼ばれてもいない持ち出しが素通りする
        let awaited_escaped_key = "import { pad } from \"./pad\";\nasync function wire() {\n  const load = module[await \"requ\\u0069re\"];\n  load(\"./stock\");\n}\n";

        assert_eq!(
            ImportSet::from_tree(&tree_of(awaited_escaped_key), Path::new("src/utils/a.ts")),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_awaiting_a_wrapped_escaped_computed_key_cannot_be_created() {
        // 包みは入れ子になる。1 段だけ剥がす形だと、`await` の中の括弧と
        // 言い当てで同じ持ち出しがもう一度素通りする
        let wrapped_escaped_key = "import { pad } from \"./pad\";\nasync function wire() {\n  const load = module[await (\"requ\\u0069re\" as string)];\n  load(\"./stock\");\n}\n";

        assert_eq!(
            ImportSet::from_tree(&tree_of(wrapped_escaped_key), Path::new("src/utils/a.ts")),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_an_awaited_require_reaches_the_same_module() {
        // 呼ばれる側が `await` に包まれても、値は `require` そのもの
        let awaited_callee = r#"import { pad } from "../utils/pad";
async function wire() {
  return (await require)("./pad");
}
"#;

        assert_eq!(
            import_set(awaited_callee, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_require_given_an_awaited_specifier_reaches_the_same_module() {
        // 指定子の側も同じ
        let awaited_specifier = r#"async function wire() {
  return require(await "./pad");
}
"#;

        assert_eq!(
            import_set(awaited_specifier, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_awaiting_a_plain_computed_key_reaches_the_same_module() {
        // 対照。読み取れる添字は落とさない
        let awaited_plain_key = r#"import { pad } from "./pad";
async function wire() {
  return config[await "title"];
}
"#;

        assert_eq!(
            import_set(awaited_plain_key, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_awaiting_require_itself_cannot_be_created() {
        // 対照。`await require` は読み込む関数そのものを取り出す。包みを剥がしても
        // 置かれている位置は変わらないので、持ち出しとして数える
        let awaited_require = r#"import { pad } from "./pad";
async function wire() {
  const loader = await require;
  return loader;
}
"#;

        assert_eq!(
            ImportSet::from_tree(&tree_of(awaited_require), Path::new("src/utils/a.ts")),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_wrapping_require_in_an_unpeeled_form_cannot_be_created() {
        // 対照に読み取れる import を 1 件置く。**一覧に無い包み**（値を選ぶ条件式）は
        // 呼び出しとして分類されないが、**綴りは束縛の側へ落ちる**のでファイルごと
        // 測れないになる。ここが崩れると、包みの形を 1 つ見つけるたびに
        // 欠けた集合で測ることになる
        let real_import_and_an_indirect_require = r#"import { pad } from "./pad";
const stock = (flag ? require : other)("./stock");
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(real_import_and_an_indirect_require),
                Path::new("src/utils/a.ts"),
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_with_a_nonliteral_first_argument_cannot_be_created() {
        // 対照に読み取れる import を 1 件置く。2 つ目以降の引数まで探す実装だと
        // "./fallback" が集合に入る（**どちらのファイルも読み込んでいない依存**）
        let real_import_and_a_fallback_argument = r#"import { pad } from "./pad";
const stock = require(target, "./fallback");
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(real_import_and_a_fallback_argument),
                Path::new("src/utils/a.ts"),
            ),
            Err(ImportsUnavailable::UnreadableDeclaration {
                line: LineNumber::from_index(1)
            })
        );
    }

    #[test]
    fn test_import_set_of_a_dynamic_import_with_attributes_reaches_the_same_module() {
        // 2 つ目の引数（読み込みの属性）は依存先ではない。最初の引数だけを見る規則が
        // この形を落とさないことを見る
        let with_attributes = r#"const pad = import("./pad", { with: { type: "json" } });"#;

        assert_eq!(
            overlap(
                with_attributes,
                "src/utils/a.ts",
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/b.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_set_of_a_require_guarded_by_typeof_reaches_the_same_module() {
        // `typeof require` も `require.resolve` も束縛を作らない。束縛を疑って
        // 落とすと、CommonJS でよくあるこの書き方のファイルが軒並み測れなくなる
        let guarded = r#"if (typeof require !== "undefined") {
  require("./pad");
}
const resolved = require.resolve("./pad");
"#;

        assert_eq!(
            overlap(
                guarded,
                "src/utils/a.ts",
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/b.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_overlap_of_files_whose_required_modules_differ_is_not_total() {
        // ESM と require が混在し、require 側の依存だけが食い違う。require を落とすと
        // 集合が両側とも "src/shared/util" だけになり、重なりが 1.00 と過大に出る
        let billing = r#"import { util } from "../shared/util";
const invoice = require("./invoice");
"#;
        let inventory = r#"import { util } from "../shared/util";
const stock = require("./stock");
"#;

        assert_eq!(
            overlap(billing, "src/billing/a.ts", inventory, "src/inventory/b.ts"),
            1.0 / 3.0
        );
    }

    #[test]
    fn test_import_overlap_of_files_sharing_only_required_modules_is_not_zero() {
        // 同じ混在でも、共有しているのが require 側の依存だけのとき。require を落とすと
        // 共有が 1 件も残らず、重なりが 0.00 と過小に出る
        let billing = r#"import { format } from "./format";
const money = require("../shared/money");
const clock = require("../shared/clock");
"#;
        let inventory = r#"import { render } from "./render";
const money = require("../shared/money");
const clock = require("../shared/clock");
"#;

        assert_eq!(
            overlap(billing, "src/billing/a.ts", inventory, "src/inventory/b.ts"),
            0.5
        );
    }

    #[test]
    fn test_import_overlap_of_files_requiring_a_bare_dot_is_not_total() {
        // 対照に共有している ESM の依存を 1 件置く。`.` を畳まないと両側の集合が
        // {"src/shared/util", "."} で揃い、**別々のディレクトリの入口**を共有している
        // ことにして重なりが 1.00 と過大に出る
        let billing = r#"import { util } from "../shared/util";
const entry = require(".");
"#;
        let inventory = r#"import { util } from "../shared/util";
const entry = require(".");
"#;

        assert_eq!(
            overlap(billing, "src/billing/a.ts", inventory, "src/inventory/b.ts"),
            1.0 / 3.0
        );
    }

    #[test]
    fn test_import_set_of_a_file_requiring_a_windows_relative_path_cannot_be_created() {
        // 特性テスト。TypeScript の文字列に逆立ちを書く唯一の方法はエスケープなので、
        // 区切りを直す前に**指定子として読み取れない**側で落ちる。
        // `ModulePath::from_specifier` の区切りを直しても、この経路の答えは変わらない
        let requires_a_windows_relative_path = r#"import { pad } from "./pad";
const entry = require(".\\stock");
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(requires_a_windows_relative_path),
                Path::new("src/billing/a.ts")
            ),
            Err(ImportsUnavailable::UnreadableDeclaration {
                line: LineNumber::from_index(1)
            })
        );
    }

    #[test]
    fn test_import_set_of_a_file_binding_an_escaped_require_cannot_be_created() {
        // 対照に読み取れる import を 1 件置く。`require` は JS の上では `require` と
        // 同じ名前だが、木が返すのは書かれた綴りのままなので、束縛を綴りで見つけられない。
        // 見つけられないまま呼び出しを採ると、**存在しない依存**を集合に入れる
        let escaped_binding = "import { pad } from \"./pad\";\nconst requ\\u0069re = helper;\nrequire(\"./stock\");\n";

        assert_eq!(
            ImportSet::from_tree(&tree_of(escaped_binding), Path::new("src/utils/a.ts")),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_loading_through_an_escaped_spelling_cannot_be_created() {
        // 対照。**呼ばれる側だけは、逆立ちがあると扱いが逆になる。** 綴りが読めるときは
        // `specifier_of` が採るので数えなくてよいが、読めないと `specifier_of` も採れない。
        // 位置だけを見て無害と答えると、**読み込みを 1 件落としたまま測れたと答える**
        let escaped_call = "import { pad } from \"./pad\";\nrequ\\u0069re(\"./stock\");\n";

        assert_eq!(
            ImportSet::from_tree(&tree_of(escaped_call), Path::new("src/utils/a.ts")),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_loading_through_an_escaped_member_cannot_be_created() {
        // 対照。要素の名前の側は、綴りが読めても読めなくても読み込みかもしれない側に残る
        // （`module` と `registry` を木の上で区別できない）。逆立ちで緩めない
        let escaped_member_call =
            "import { pad } from \"./pad\";\nmodule.requ\\u0069re(\"./stock\");\n";

        assert_eq!(
            ImportSet::from_tree(&tree_of(escaped_member_call), Path::new("src/utils/a.ts")),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_naming_an_escaped_type_require_reaches_the_same_module() {
        // 逆立ちを書けるのは名前なので、型空間にも書ける。型は実行時の名前を作らないので、
        // **綴りが読めるかに関わらず**読み込む関数を持ち出せない
        let escaped_type_name = "import { pad } from \"./pad\";\ntype requ\\u0069re = string;\nlet handle: requ\\u0069re;\n";

        assert_eq!(
            import_set(escaped_type_name, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_naming_an_escaped_object_key_reaches_the_same_module() {
        // 欄を作る側も、綴りが読めるかに関わらず束縛でも参照でもない
        let escaped_object_key =
            "import { pad } from \"./pad\";\nconst options = { requ\\u0069re: false };\n";

        assert_eq!(
            import_set(escaped_object_key, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_labelling_a_statement_with_an_escaped_name_reaches_the_same_module()
     {
        // ラベルは別の名前空間にいるので、綴りが読めるかに関わらず値にならない
        let escaped_label =
            "import { pad } from \"./pad\";\nrequ\\u0069re: { break requ\\u0069re; }\n";

        assert_eq!(
            import_set(escaped_label, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_with_a_backslash_outside_a_name_reaches_the_same_module() {
        // 対照。逆立ちが名前の中に無い（正規表現・コメント・文字列の中）ファイルまで
        // 測れなくすると、`\d` を書いただけのファイルが軒並み落ちる
        let backslashes_outside_names = r#"// a \ backslash in a comment
const digits = /\d+/;
const escaped = "a\nb";
import { pad } from "./pad";
"#;

        assert_eq!(
            import_set(backslashes_outside_names, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_with_a_backslash_in_jsx_text_reaches_the_same_module() {
        // 対照。JSX の地の文は名前ではないので、逆立ちがあっても綴りは比べられる。
        // 名前でない葉まで名前として数えると、`C:\users` と書いた画面が軒並み落ちる
        let backslash_in_jsx_text = r#"import { pad } from "./pad";

export const Path = () => <p>C:\users</p>;
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tsx_tree_of(backslash_in_jsx_text),
                Path::new("src/utils/formatDate.tsx"),
            ),
            Ok(import_set(
                IMPORTS_PAD_FROM_PARENT,
                "src/report/dateHelper.ts"
            ))
        );
    }

    #[test]
    fn test_import_set_of_a_file_asserting_a_wrapped_type_named_require_reaches_the_same_module() {
        // 言い当ての型が包まれても型空間にいることは変わらない。直接の親だけを見ると
        // `require[]` や `ns.require` で漏れる
        let require_wrapped_in_an_assertion = r#"import { pad } from "./pad";

type require = string;
export const listed = value as require[];
export const qualified = other as ns.require;
"#;

        assert_eq!(
            import_set(require_wrapped_in_an_assertion, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_asserting_a_class_named_require_cannot_be_created() {
        // 対照。クラス式の名前は `type_identifier` だが**実行時の束縛を作る**。
        // 言い当ての下にあるからと型空間へ寄せると、持ち出しを見落とす
        let class_named_require_in_an_assertion = r#"import { pad } from "./pad";
const held = (class require {}) as unknown;
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(class_named_require_in_an_assertion),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_constructing_a_computed_key_cannot_be_created() {
        // `new` でも読み込みは起きる。呼び出しの形だけを見ると、綴りを読めない添字が素通りする
        let computed_key_constructed = r#"import { pad } from "./pad";
new module["re" + "quire"]("./stock");
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(computed_key_constructed),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_constructing_require_cannot_be_created() {
        // 対照。`new require("./x")` は `specifier_of` が採らないので、
        // 呼ばれた側と同じに扱うと依存が黙って落ちる
        let require_constructed = r#"import { pad } from "./pad";
const held = new require("./stock");
"#;

        assert_eq!(
            ImportSet::from_tree(&tree_of(require_constructed), Path::new("src/utils/a.ts")),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_computing_a_static_object_key_reaches_the_same_module() {
        // 計算された欄でも、綴りが読める文字列なら欄の名前でしかない
        let require_as_a_computed_object_key = r#"import { pad } from "./pad";
const options = { ["require"]: false, retries: 2 };
"#;

        assert_eq!(
            import_set(require_as_a_computed_object_key, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_computing_an_object_key_from_require_cannot_be_created() {
        // 対照。計算された欄に**名前**を書くと、それは値の参照。
        // 文字列かどうかを見ずに剥がすと、読み込む関数の持ち出しを見落とす
        let require_named_in_a_computed_key = r#"import { pad } from "./pad";
const options = { [require]: false };
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(require_named_in_a_computed_key),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_labelling_a_statement_require_reaches_the_same_module() {
        // ラベルは名前だが**別の名前空間**にいる。値を束縛も参照もしない
        let require_as_a_statement_label = r#"import { pad } from "./pad";

export function scan() {
  require: for (const row of rows) {
    break require;
  }
}
"#;

        assert_eq!(
            import_set(require_as_a_statement_label, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_namespacing_a_jsx_attribute_with_require_reaches_the_same_module()
    {
        // 名前空間つきの属性は、前半も後半も属性の名前の一部でしかない
        let require_in_a_namespaced_attribute = r#"import { pad } from "./pad";

export const Prefixed = () => <Widget require:mode="x" />;
export const Suffixed = () => <Widget ns:require="x" />;
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tsx_tree_of(require_in_a_namespaced_attribute),
                Path::new("src/utils/formatDate.tsx"),
            ),
            Ok(import_set(
                IMPORTS_PAD_FROM_PARENT,
                "src/report/dateHelper.ts"
            ))
        );
    }

    #[test]
    fn test_import_set_of_a_file_namespacing_a_jsx_tag_with_require_reaches_the_same_module() {
        // タグの名前空間も同じ。**指摘は属性だけだったが、タグ側も同じ形で漏れていた**
        let require_in_a_namespaced_tag = r#"import { pad } from "./pad";

export const Prefixed = () => <require:tag />;
export const Suffixed = () => <ns:require />;
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tsx_tree_of(require_in_a_namespaced_tag),
                Path::new("src/utils/formatDate.tsx"),
            ),
            Ok(import_set(
                IMPORTS_PAD_FROM_PARENT,
                "src/report/dateHelper.ts"
            ))
        );
    }

    #[test]
    fn test_import_set_of_a_file_with_a_require_jsx_tag_reaches_the_same_module() {
        // 小文字の JSX タグは組み込み要素の名前で、文字列としてコンパイルされる。
        // 束縛も参照も作らないので、画面に置くだけで依存が落ちてはいけない
        let require_as_a_jsx_tag = r#"import { pad } from "./pad";

export const Slot = () => <require>x</require>;
export const Empty = () => <require />;
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tsx_tree_of(require_as_a_jsx_tag),
                Path::new("src/utils/formatDate.tsx"),
            ),
            Ok(import_set(
                IMPORTS_PAD_FROM_PARENT,
                "src/report/dateHelper.ts"
            ))
        );
    }

    #[test]
    fn test_import_set_of_a_file_declaring_a_require_overload_reaches_the_same_module() {
        // 本体を持たない宣言の引数は消える。実装の側だけが実行時の引数を作る
        let require_in_an_overload_declaration = r#"import { pad } from "./pad";

export function parse(require: string): void;
export function parse(value: string): void {}
"#;

        assert_eq!(
            import_set(
                require_in_an_overload_declaration,
                "src/utils/formatDate.ts"
            ),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_implementing_a_require_overload_cannot_be_created() {
        // 対照。名前が `require` の実装は `function_declaration` の下に来るので、
        // 宣言の側を型空間へ寄せても測れない側に残る
        let require_declared_and_implemented = r#"import { pad } from "./pad";
export function require(value: string): void;
export function require(value: string) {}
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(require_declared_and_implemented),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_reading_a_computed_require_member_reaches_the_same_module() {
        // `require["resolve"]` は `require.resolve` と同じ要素。綴りが読めるなら
        // 書き方が違うだけで読み込みは起こさない
        let non_loading_member_read_by_a_computed_key = r#"import { pad } from "./pad";
require["resolve"]("./pad");
require["cache"];
const { pad: padded } = require("./pad");
"#;

        assert_eq!(
            import_set(
                non_loading_member_read_by_a_computed_key,
                "src/utils/formatDate.ts"
            ),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_reading_a_loading_member_by_key_cannot_be_created() {
        // 対照。`require["extensions"]` は読み込みを行う関数そのもの。添字の綴りを
        // 見ずに要素アクセスを外すと、一覧に無い要素まで測れる側へ落ちる
        let loading_member_read_by_a_computed_key = r#"import { pad } from "./pad";
const extend = require["extensions"];
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(loading_member_read_by_a_computed_key),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_reading_a_substituted_computed_key_reaches_the_same_module() {
        // 置換を持つ添字は**実行時に決まる**ので、`config[key]` と同じ扱い。
        // 呼ばれていない読み出しで依存を落とすと、ごく普通のコードが測れなくなる
        let substituted_computed_read = r#"import { pad } from "./pad";
const title = config[`title-${locale}`];
"#;

        assert_eq!(
            import_set(substituted_computed_read, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_holding_an_escaped_computed_key_cannot_be_created() {
        // 対照。エスケープを含む添字は**綴りが静的に決まっている**ので、呼ばれる前に
        // 持ち出されても `require` かどうかを決められない
        let escaped_computed_key_held = r#"import { pad } from "./pad";
const load = module["require"];
load("./stock");
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(escaped_computed_key_held),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_with_require_in_jsx_text_reaches_the_same_module() {
        // JSX の地の文は名前を書けない場所。画面に `require` と表示するだけで
        // そのファイルの依存が測れなくなると、無関係な文言で判定が落ちる
        let require_in_jsx_text = r#"import { pad } from "./pad";

export const Help = () => <p>require</p>;
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tsx_tree_of(require_in_jsx_text),
                Path::new("src/utils/formatDate.tsx"),
            ),
            Ok(import_set(
                IMPORTS_PAD_FROM_PARENT,
                "src/report/dateHelper.ts"
            ))
        );
    }

    #[test]
    fn test_import_set_of_a_file_matching_require_in_a_regex_reaches_the_same_module() {
        // 正規表現の中身も名前を書けない場所。綴りが一致しても束縛も読み込みも作らない
        let require_in_a_regex = r#"import { pad } from "./pad";
const mentionsRequire = /require/;
"#;

        assert_eq!(
            import_set(require_in_a_regex, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_naming_a_jsx_attribute_require_reaches_the_same_module() {
        // JSX の属性の名前は欄の名前と同じで、束縛も参照も作らない。
        // 名前として数えると、`require` という prop を持つ画面が軒並み落ちる
        let require_as_a_jsx_attribute = r#"import { pad } from "./pad";

export const View = () => <Widget require={false} label="x" />;
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tsx_tree_of(require_as_a_jsx_attribute),
                Path::new("src/utils/formatDate.tsx"),
            ),
            Ok(import_set(
                IMPORTS_PAD_FROM_PARENT,
                "src/report/dateHelper.ts"
            ))
        );
    }

    #[test]
    fn test_import_set_of_a_file_shorthanding_a_jsx_attribute_require_cannot_be_created() {
        // 対照。省略記法（`{...}` の中で名前を渡す）は**名前を参照する**ので、
        // 読み込む関数そのものを prop として持ち出せる
        let require_passed_as_a_jsx_value = r#"import { pad } from "./pad";

export const View = () => <Widget loader={require} />;
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tsx_tree_of(require_passed_as_a_jsx_value),
                Path::new("src/utils/a.tsx"),
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_writing_require_as_a_jsx_attribute_value_cannot_be_created() {
        // 対照。属性の値の文字列は名前の位置と**同じ親の直下**に来るので、
        // 位置を見ずに属性の下なら外すと、この綴りまで落ちる
        let require_as_a_jsx_attribute_value = r#"import { pad } from "./pad";

export const View = () => <Widget label="require" />;
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tsx_tree_of(require_as_a_jsx_attribute_value),
                Path::new("src/utils/a.tsx"),
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_implementing_a_type_named_require_reaches_the_same_module() {
        // `implements` の名前は型空間にしかいない。実行時には消えるので、
        // 束縛し直しも読み込みも起こしえない
        let require_in_an_implements_clause = r#"import { pad } from "./pad";
import type { require } from "./loader";

class Worker implements require {}
"#;

        assert_eq!(
            import_set(require_in_an_implements_clause, "src/utils/a.ts"),
            import_set(IMPORTS_PAD_AND_LOADER, "src/utils/a.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_extending_a_class_named_require_cannot_be_created() {
        // 対照。`extends` の側は**値**（実行時に評価される式）なので、
        // 型だけの `implements` と同じに扱うと持ち出しを見落とす
        let require_in_an_extends_clause = r#"import { pad } from "./pad";

class Worker extends require {}
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(require_in_an_extends_clause),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_declaring_require_in_a_type_reaches_the_same_module() {
        // 型の中の名前は、実行時の `require` を束縛し直しも読み込みもしない。
        // 数えると、`require` を持つ型を書いただけで、書いてある import まで測れなくなる
        let require_declared_in_types = r#"import { pad } from "./pad";

interface Loader { require(path: string): void }
type Reader = { require: (path: string) => void };
abstract class Base { abstract require(path: string): void; }
"#;

        assert_eq!(
            import_set(require_declared_in_types, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    /// 型だけの輸入と値の輸入は、**名前のあるノードだけを見ると同じ木**になる。
    /// 印は名前のないノードなので、そこまで見ないと区別できない。
    const IMPORTS_PAD_AND_LOADER: &str = r#"import { pad } from "./pad";
import { load } from "./loader";
"#;

    #[test]
    fn test_import_set_of_a_file_asserting_a_type_named_require_reaches_the_same_module() {
        // 型の側の綴りは型空間にしかいない。`<require>value` は `type_arguments` の
        // 下に来るので既に外れており、残っていたのは `as` と `satisfies` の 2 つ
        let require_named_in_assertions = r#"import { pad } from "./pad";

type require = string;
export const asserted = value as require;
export const checked = value satisfies require;
"#;

        assert_eq!(
            import_set(require_named_in_assertions, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_asserting_a_type_onto_require_cannot_be_created() {
        // 対照。同じ `as_expression` でも**値の側**は読み込む関数そのもので、
        // 捕まえて持ち出せる。木の上では型の側が `type_identifier`、値の側が
        // `identifier` と種別が違う
        let require_captured_through_an_assertion = r#"import { pad } from "./pad";
const captured = require as NodeRequire;
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(require_captured_through_an_assertion),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_re_exporting_a_namespace_as_require_reaches_the_same_modules() {
        // 名前空間の再輸出。別名は `export_specifier` ではなく `namespace_export` の
        // 下に来るが、ローカルの名前を作らないのは同じ
        let namespace_re_exported_as_require = r#"import { pad } from "./pad";
export * as require from "./loader";
"#;

        assert_eq!(
            import_set(namespace_re_exported_as_require, "src/utils/a.ts"),
            import_set(IMPORTS_PAD_AND_LOADER, "src/utils/a.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_quoting_an_object_key_require_reaches_the_same_module() {
        // 引用符で囲んだ欄の名前も欄の名前。木の上では `string` を 1 つ挟むだけで、
        // 束縛でも参照でもないことは変わらない
        let require_as_a_quoted_object_key = r#"import { pad } from "./pad";
const options = { "require": false, retries: 2 };
"#;

        assert_eq!(
            import_set(require_as_a_quoted_object_key, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_naming_an_object_key_require_reaches_the_same_module() {
        // オブジェクトの欄の名前は束縛でも参照でもない。あとで読むときは `options.require` の
        // 要素アクセスになるので、そちらで数えれば足りる
        let require_as_an_object_key = r#"import { pad } from "./pad";
const options = { require: false };
"#;

        assert_eq!(
            import_set(require_as_an_object_key, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_destructuring_the_require_key_cannot_be_created() {
        // 分解して受ける側は欄を**読む**。`const load = module.require;` と同じ持ち出しなので、
        // 書く側（`{ require: false }`）と同じに扱うと読み込みを 1 件落としたまま集合を返す
        let require_destructured_out_of_an_object = r#"import { pad } from "./pad";
const { require: load } = module;
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(require_destructured_out_of_an_object),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_destructuring_the_require_key_in_a_parameter_cannot_be_created() {
        // 引数で分解しても読む側であることは変わらない。祖先の鎖だけが違う
        let require_destructured_in_a_parameter = r#"import { pad } from "./pad";
function wire({ require: load }) {}
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(require_destructured_in_a_parameter),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_destructuring_a_quoted_require_key_cannot_be_created() {
        // 引用符で囲んでも読む側。書く側の `{ "require": false }` と綴りは同じ
        let quoted_require_destructured = r#"import { pad } from "./pad";
const { "require": load } = registry;
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(quoted_require_destructured),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_destructuring_a_computed_require_key_cannot_be_created() {
        // 計算された欄でも読む側。書く側の `{ ["require"]: false }` と綴りは同じ
        let computed_require_destructured = r#"import { pad } from "./pad";
const { ["require"]: load } = registry;
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(computed_require_destructured),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_loading_through_a_computed_key_cannot_be_created() {
        // 添字が文字列リテラルですらないと、**どのノードも `require` を綴らない**。
        // 綴りの一致では見つけられないので、呼ばれている添字は読み取れない側へ落とす
        let require_spelled_by_a_computed_key = r#"import { pad } from "./pad";
module["re" + "quire"]("./stock");
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(require_spelled_by_a_computed_key),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_calling_a_readable_computed_key_reaches_the_same_module() {
        // 対照。添字が読み取れれば綴りは分かるので、`require` でないと言い切れる。
        // 呼ばれているだけで落とすと、ごく普通の要素アクセスまで測れなくなる
        let readable_computed_key_called = r#"import { pad } from "./pad";
handlers["load"]("./stock");
"#;

        assert_eq!(
            import_set(readable_computed_key_called, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_reading_an_uninvoked_computed_key_reaches_the_same_module() {
        // 対照。呼ばれていない添字は読み込みを起こさない。呼ばれているかを見ずに
        // 落とすと、ごく普通の要素アクセス（`arr[i]` / `map[key]`）まで測れなくなる
        let uninvoked_computed_key = r#"import { pad } from "./pad";
const value = config[key];
"#;

        assert_eq!(
            import_set(uninvoked_computed_key, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_passing_require_to_another_call_cannot_be_created() {
        // 対照。指定子として読まれるのは依存を受ける呼び出しの引数だけ。
        // 呼び出しなら何でも除外すると、ただの文字列の位置で綴りを見逃す
        let require_passed_to_another_call = r#"import { pad } from "./pad";
load("require");
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(require_passed_to_another_call),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_spelling_require_in_a_later_argument_cannot_be_created() {
        // 対照。指定子として読まれるのは**最初の引数だけ**なので、2 つ目以降の
        // `require` は名前として数える側に残る。読む位置を広げると綴りを見逃す
        let require_spelled_in_a_later_argument = r#"const { pad } = require("./pad", "require");
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(require_spelled_in_a_later_argument),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_depending_on_a_module_named_require_reaches_both_modules() {
        // 依存先の綴りが `require` でも、それは**依存先の名前**であって
        // このファイルの名前ではない。輸入の `source` の欄と、呼び出しの最初の引数は
        // どちらも `specifier_of` が既に指定子として読む位置
        let imports_a_module_named_require = r#"import { pad } from "./pad";
import loader from "require";
"#;
        let requires_a_module_named_require = r#"const { pad } = require("./pad");
const loader = require("require");
"#;

        assert_eq!(
            import_set(imports_a_module_named_require, "src/utils/a.ts"),
            import_set(requires_a_module_named_require, "src/utils/a.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_writing_require_as_an_object_value_cannot_be_created() {
        // 対照。引用符つきの欄の名前と**同じ木の形**（`string` の下の `string_fragment`）
        // だが、こちらは `pair` の `value` の側。欄かどうかを欄の名前まで見ずに
        // 「`pair` の下の文字列」で決めると、こちらまで外れる
        let require_as_an_object_value = r#"import { pad } from "./pad";
const options = { mode: "require" };
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(require_as_an_object_value),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_shorthanding_require_into_an_object_cannot_be_created() {
        // 対照。省略記法は**名前を参照する**ので、読み込む関数そのものを
        // オブジェクトへ持ち出せる。欄の名前と同じに扱うと持ち出しを見落とす
        let require_shorthanded_into_an_object = r#"import { pad } from "./pad";
const carrier = { require };
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(require_shorthanded_into_an_object),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_passing_require_as_a_type_argument_reaches_the_same_module() {
        // 型引数も型空間にしかいない。値の引数（`arguments`）とは別の種別
        let require_passed_as_a_type_argument = r#"import { pad } from "./pad";

type require = string;
export const parsed = consume<require>();
"#;

        assert_eq!(
            import_set(require_passed_as_a_type_argument, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_re_exporting_require_under_another_name_reaches_the_same_modules()
    {
        // 輸出元を持つ輸出（再輸出）はローカルの名前を 1 つも作らない。
        // 綴りは相手のモジュールの輸出名であって、このファイルの束縛ではない
        let require_re_exported = r#"import { pad } from "./pad";
export { require as load } from "./loader";
"#;

        assert_eq!(
            import_set(require_re_exported, "src/utils/a.ts"),
            import_set(IMPORTS_PAD_AND_LOADER, "src/utils/a.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_exporting_a_local_name_as_require_reaches_the_same_module() {
        // 輸出元が無くても、**別名の側**はローカルの名前を作らない。輸出される名前で
        // しかないので、束縛でも参照でもない
        let local_exported_as_require = r#"import { pad } from "./pad";
const load = 1;
export { load as require };
"#;

        assert_eq!(
            import_set(local_exported_as_require, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_importing_a_string_named_require_reaches_the_same_modules() {
        // 輸入元の名前は文字列でも書ける（ES2022）。木の上では `string` を 1 つ挟むだけで、
        // 実行時の名前になるのが別名だけなのは変わらない
        let string_named_import = r#"import { pad } from "./pad";
import { "require" as load } from "./loader";
"#;

        assert_eq!(
            import_set(string_named_import, "src/utils/a.ts"),
            import_set(IMPORTS_PAD_AND_LOADER, "src/utils/a.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_exporting_a_local_name_as_a_string_require_reaches_the_same_module()
     {
        // 別名の側が文字列でも、輸出される名前でしかない
        let local_exported_as_string_require = r#"import { pad } from "./pad";
const load = 1;
export { load as "require" };
"#;

        assert_eq!(
            import_set(local_exported_as_string_require, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_re_exporting_a_string_named_require_reaches_the_same_modules() {
        // 再輸出の輸入元が文字列でも、このファイルの束縛は 1 つも作らない
        let string_named_re_export = r#"import { pad } from "./pad";
export { "require" as load } from "./loader";
"#;

        assert_eq!(
            import_set(string_named_re_export, "src/utils/a.ts"),
            import_set(IMPORTS_PAD_AND_LOADER, "src/utils/a.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_exporting_a_local_require_cannot_be_created() {
        // 対照。輸出元を持たない輸出は**このファイルの束縛**を指す。CommonJS の
        // ファイルならそれは読み込む関数そのもので、輸出すると持ち出される
        let local_require_exported = r#"import { pad } from "./pad";
export { require as load };
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(local_require_exported),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_loading_through_an_escaped_computed_key_cannot_be_created() {
        // 添字に書いた綴りを読み取れないと、それが `require` かどうかを決められない。
        // 逆立ちを文字列の中だから無害と決めると、**読み込みを 1 件落としたまま
        // 測れたと答える**（この PR で唯一、安全でない側へ倒れていた形）
        let escaped_computed_key =
            "import { pad } from \"./pad\";\nmodule[\"requ\\u0069re\"](\"./stock\");\n";

        assert_eq!(
            ImportSet::from_tree(&tree_of(escaped_computed_key), Path::new("src/utils/a.ts")),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_wrapping_an_escaped_computed_key_cannot_be_created() {
        // 包みは値を変えないので、添字の位置は包みの外で見る。直下だけを見ると
        // **呼ばれてもいない添字が素通りし、読み込みを 1 件落としたまま測れたと答える**
        let wrapped_escaped_key = "import { pad } from \"./pad\";\nconst load = module[(\"requ\\u0069re\")];\nload(\"./stock\");\n";

        assert_eq!(
            ImportSet::from_tree(&tree_of(wrapped_escaped_key), Path::new("src/utils/a.ts")),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_asserting_an_escaped_computed_key_cannot_be_created() {
        // 括弧だけではない。`as` も値を変えない包みで、同じ穴が開いていた
        let asserted_escaped_key = "import { pad } from \"./pad\";\nconst load = module[(\"requ\\u0069re\") as string];\nload(\"./stock\");\n";

        assert_eq!(
            ImportSet::from_tree(&tree_of(asserted_escaped_key), Path::new("src/utils/a.ts")),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_asserting_an_escaped_computed_key_with_angles_cannot_be_created() {
        // 型を先に書く言い当ても値を変えない包み。中の式が最初の子ではないので
        // 剥がす側の一覧には入れられないが、**位置を見るだけの上向きの走査には要らない**
        let angle_asserted_escaped_key = "import { pad } from \"./pad\";\nconst load = module[<string>\"requ\\u0069re\"];\nload(\"./stock\");\n";

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(angle_asserted_escaped_key),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_destructuring_an_escaped_computed_key_cannot_be_created() {
        // 分解して受ける側は欄を**読む**。綴りを読み取れないと、それが `require` か
        // どうかを決められないのは添字と同じ
        let escaped_computed_pattern_key = "import { pad } from \"./pad\";\nconst { [\"requ\\u0069re\"]: load } = module;\nload(\"./stock\");\n";

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(escaped_computed_pattern_key),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_destructuring_an_escaped_string_key_cannot_be_created() {
        // 計算された欄を挟まない綴りの欄も、読む側であることは変わらない
        let escaped_string_pattern_key = "import { pad } from \"./pad\";\nconst { \"requ\\u0069re\": load } = module;\nload(\"./stock\");\n";

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(escaped_string_pattern_key),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_destructuring_an_escaped_key_in_a_parameter_cannot_be_created() {
        // 引数で分解しても読む側。祖先の鎖だけが違う
        let escaped_key_in_a_parameter = "import { pad } from \"./pad\";\nfunction wire({ [\"requ\\u0069re\"]: load }) { load(\"./stock\"); }\n";

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(escaped_key_in_a_parameter),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_naming_an_escaped_computed_object_key_reaches_the_same_module() {
        // 対照。**書く側**は欄を作るだけで、綴りが読めるかに関わらず読み込みを起こさない。
        // 読む側と同じに扱うと、`{ ["a\u0062c"]: 1 }` を書いただけのファイルが落ちる
        let escaped_computed_literal_key =
            "import { pad } from \"./pad\";\nconst options = { [\"requ\\u0069re\"]: false };\n";

        assert_eq!(
            import_set(escaped_computed_literal_key, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_substituting_into_an_escaped_key_reaches_the_same_module() {
        // 置換が 1 つでもあれば、綴りは**実行時に決まる**。エスケープが混ざっていても
        // 書いた時点では決まらないので、`config[key]` と同じく呼ばれているときだけ見る
        let escaped_key_with_a_substitution =
            "import { pad } from \"./pad\";\nconst title = config[`title\\u0069-${locale}`];\n";

        assert_eq!(
            import_set(escaped_key_with_a_substitution, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_reading_an_empty_computed_key_reaches_the_same_module() {
        // 対照。中身が空の綴りは読み取れないが、**エスケープも置換も無い**。
        // 読み取れないだけで静的と決めると、`module[""]` を書いただけのファイルが落ちる
        let empty_computed_key = r#"import { pad } from "./pad";
const value = module[""];
"#;

        assert_eq!(
            import_set(empty_computed_key, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_escaping_a_key_without_substituting_cannot_be_created() {
        // 対照。置換が無ければ綴りは書いた時点で決まる。呼ばれていなくても測れない
        let escaped_key_without_a_substitution = "import { pad } from \"./pad\";\nconst load = module[`requ\\u0069re`];\nload(\"./stock\");\n";

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(escaped_key_without_a_substitution),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_invoking_a_substituted_escaped_key_cannot_be_created() {
        // 対照。実行時に決まる綴りでも、**呼ばれていれば**読み込みを落とせない
        let invoked_key_with_a_substitution =
            "import { pad } from \"./pad\";\nmodule[`requ\\u0069re${x}`](\"./stock\");\n";

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(invoked_key_with_a_substitution),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_wrapping_a_plain_computed_key_reaches_the_same_module() {
        // 対照。包みの外まで戻すのを読み取れる添字にも広げると、`config[("title")]` を
        // 書いただけのファイルまで落ちる
        let wrapped_plain_key = r#"import { pad } from "./pad";
const title = config[("title")];
"#;

        assert_eq!(
            import_set(wrapped_plain_key, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_reading_a_plain_computed_key_reaches_the_same_module() {
        // 対照。読み取れる添字まで測れなくすると、`config["title"]` を書いただけの
        // ファイルが軒並み落ちる
        let plain_computed_key = r#"import { pad } from "./pad";
const title = config["title"];
"#;

        assert_eq!(
            import_set(plain_computed_key, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_importing_require_under_another_name_reaches_the_same_modules() {
        // 別名を付けた輸入で実行時の名前になるのは別名のほうだけ。輸入元の綴りは
        // 束縛を作らないので、数えると測れるファイルが減る
        let require_imported_under_another_name = r#"import { pad } from "./pad";
import { require as load } from "./loader";
"#;

        assert_eq!(
            import_set(require_imported_under_another_name, "src/utils/a.ts"),
            import_set(IMPORTS_PAD_AND_LOADER, "src/utils/a.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_aliasing_an_import_to_require_cannot_be_created() {
        // 対照。別名のほうが `require` なら実行時の名前を作る。輸入元と別名を
        // 見分けずに外すと、この形まで測れる側へ戻ってしまう
        let import_aliased_to_require = r#"import { pad } from "./pad";
import { load as require } from "./loader";
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(import_aliased_to_require),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_naming_a_generic_parameter_require_reaches_the_same_module() {
        // 型変数の名前も型空間にしかいない。実物の関数・クラスに付いていても同じ
        let require_named_as_a_generic_parameter = r#"import { pad } from "./pad";

export function parse<require>(value: require): require { return value; }
"#;

        assert_eq!(
            import_set(
                require_named_as_a_generic_parameter,
                "src/utils/formatDate.ts"
            ),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_importing_require_only_as_a_type_reaches_the_same_modules() {
        // 型だけの輸入は実行時の名前を作らない。束縛と数えると、`require` という型を
        // 輸入しただけで書いてある依存が全部落ちる
        let require_imported_as_a_type = r#"import { pad } from "./pad";
import type { require } from "./loader";
"#;

        assert_eq!(
            import_set(require_imported_as_a_type, "src/utils/a.ts"),
            import_set(IMPORTS_PAD_AND_LOADER, "src/utils/a.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_marking_require_as_a_type_in_the_clause_reaches_the_same_modules()
    {
        // 印が輸入の文ではなく個々の名前に付く形。木の上では印の置き場所が違う
        let require_marked_as_a_type = r#"import { pad } from "./pad";
import { type require } from "./loader";
"#;

        assert_eq!(
            import_set(require_marked_as_a_type, "src/utils/a.ts"),
            import_set(IMPORTS_PAD_AND_LOADER, "src/utils/a.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_importing_require_as_a_value_cannot_be_created() {
        // 対照。値として輸入した `require` は実行時の名前を作るので、外してはいけない
        let require_imported_as_a_value = r#"import { pad } from "./pad";
import { require } from "./loader";
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(require_imported_as_a_value),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_aliasing_an_import_named_type_to_require_cannot_be_created() {
        // 対照。綴りに `type` が現れるが印ではない。**`type` という名前を `require` へ
        // 別名にした値の輸入**なので、綴りで印を探す実装だと外れてしまう
        let type_aliased_to_require = r#"import { pad } from "./pad";
import { type as require } from "./loader";
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(type_aliased_to_require),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_aliasing_require_as_a_type_reaches_the_same_module() {
        // 型の名前としての `require` は型空間にしかいない。実行時の読み込みとは
        // 別の名前空間なので、束縛と数えると型を 1 つ書いただけで測れなくなる
        let require_aliased_as_a_type = r#"import { pad } from "./pad";

type require = string;
let value: require;
"#;

        assert_eq!(
            import_set(require_aliased_as_a_type, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_declaring_require_as_an_interface_reaches_the_same_module() {
        // インタフェースの名前も型空間にしかいない
        let require_declared_as_an_interface = r#"import { pad } from "./pad";

interface require { readonly kind: string }
"#;

        assert_eq!(
            import_set(require_declared_as_an_interface, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_declaring_a_class_named_require_cannot_be_created() {
        // 対照。クラスの名前は木の上で型の名前と**同じ `type_identifier`** になるが、
        // 実行時の束縛を作る。種別だけで外すと、この形まで測れる側へ戻ってしまう
        let class_named_require = r#"import { pad } from "./pad";

class require {
  load(path: string): string { return path; }
}
"#;

        assert_eq!(
            ImportSet::from_tree(&tree_of(class_named_require), Path::new("src/utils/a.ts")),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_naming_require_in_a_type_parameter_reaches_the_same_module() {
        // 型の中の引数の名前も値を持たない。要素の宣言だけを外すと、この形が残る
        let require_named_in_type_parameters = r#"import { pad } from "./pad";

type Handler = (require: string) => void;
type Factory = new (require: string) => void;
interface Callable { (require: string): void }
"#;

        assert_eq!(
            import_set(require_named_in_type_parameters, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_taking_require_as_a_function_parameter_cannot_be_created() {
        // 対照。実物の関数の引数は名前を束縛するので、外してはいけない。型の中の引数を
        // 外すのに引数の種別だけで決めると、この形まで測れる側へ戻ってしまう
        let require_taken_as_a_parameter = r#"import { pad } from "./pad";

export function load(require: (path: string) => string): string {
  return require("./stock");
}
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(require_taken_as_a_parameter),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_require_given_a_parenthesized_specifier_reaches_the_same_module() {
        // 包みは値を変えない。呼ばれる側だけ剥がして引数を剥がさないと、
        // 綴りの決まった指定子を読み取れないことにしてしまう
        let parenthesized_specifier = r#"const pad = require(("./pad"));
"#;

        assert_eq!(
            import_set(parenthesized_specifier, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_require_given_an_angle_asserted_specifier_reaches_the_same_module() {
        // 型を先に書く言い当ても値を変えない包み。`as` の形だけ剥がして
        // こちらを剥がさないと、同じ指定子が書き方の違いで読み取れなくなる
        let angle_asserted_specifier = r#"const pad = require(<string>"./pad");
"#;

        assert_eq!(
            import_set(angle_asserted_specifier, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_require_given_a_sequenced_specifier_reaches_the_same_module() {
        // 順に評価して最後を返す並びも、値は最後の子そのもの
        let sequenced_specifier = r#"const pad = require((0, "./pad"));
"#;

        assert_eq!(
            import_set(sequenced_specifier, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_sequenced_require_reaches_the_same_module() {
        // 呼ばれる側が並びでも、値は `require` そのもの
        let sequenced_callee = r#"const pad = (0, require)("./pad");
"#;

        assert_eq!(
            import_set(sequenced_callee, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_sequencing_an_escaped_computed_key_cannot_be_created() {
        // 並びの**最後**の値が添字になる。剥がさないと、呼ばれてもいない持ち出しが
        // 素通りして読み込みを 1 件落とす
        let sequenced_escaped_key = "import { pad } from \"./pad\";\nconst load = module[(0, \"requ\\u0069re\")];\nload(\"./stock\");\n";

        assert_eq!(
            ImportSet::from_tree(&tree_of(sequenced_escaped_key), Path::new("src/utils/a.ts")),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_sequencing_a_key_before_an_escaped_string_reaches_the_same_module()
    {
        // 対照。**最後でない**値は添字にならない。並びを丸ごと剥がすと、
        // 添字ですらない綴りを添字と取り違える
        let escaped_string_before_the_key =
            "import { pad } from \"./pad\";\nconst value = module[(\"requ\\u0069re\", 0)];\n";

        assert_eq!(
            import_set(escaped_string_before_the_key, "src/utils/formatDate.ts"),
            import_set(IMPORTS_PAD_FROM_PARENT, "src/report/dateHelper.ts")
        );
    }

    #[test]
    fn test_import_set_of_a_file_defining_a_require_method_on_a_class_cannot_be_created() {
        // 対照。実装を伴う定義は型ではないので、外してはいけない。型の中の名前を外すのに
        // 要素の名前を丸ごと外すと、この形まで測れる側へ戻ってしまう
        let require_defined_on_a_class = r#"import { pad } from "./pad";

class Loader { require(path: string): string { return path; } }
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(require_defined_on_a_class),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_of_a_file_exporting_an_import_equals_require_cannot_be_created() {
        // 対照に読み取れる import を 1 件置く。tree-sitter はこの形の右辺を
        // `import_require_clause` として出さず、`require` が呼ばれる側でない
        // `identifier` として残る。宣言として見えないまま残りだけで測ると、
        // **欠けた集合を揃った集合として扱う**
        let exported_import_equals = r#"import { pad } from "./pad";
export import dep = require("./dep");
"#;

        assert_eq!(
            ImportSet::from_tree(
                &tree_of(exported_import_equals),
                Path::new("src/utils/a.ts")
            ),
            Err(ImportsUnavailable::ReboundSpelling)
        );
    }

    #[test]
    fn test_import_set_ignores_an_export_that_has_no_source() {
        // 対照に `from` 付きの export を 1 件置く。export の中の文字列を無条件に拾う
        // 実装だと集合に "./stock" が入り、重なりが 0.5 に落ちる
        let re_export_and_a_decoy = r#"export { pad } from "./pad";
export const fallback = "./stock";
"#;

        assert_eq!(
            overlap(
                re_export_and_a_decoy,
                "src/utils/a.ts",
                IMPORTS_PAD_FROM_SIBLING,
                "src/utils/b.ts",
            ),
            1.0
        );
    }

    #[test]
    fn test_import_set_ignores_an_import_written_inside_a_comment() {
        let commented_out = r#"// import { pad } from "./pad";
const value = 1;
"#;

        assert_eq!(
            ImportSet::from_tree(&tree_of(commented_out), Path::new("src/utils/a.ts")),
            Err(ImportsUnavailable::NoDeclarations)
        );
    }

    #[test]
    fn test_import_set_of_a_source_without_imports_cannot_be_created() {
        // 「import が無い」を空の集合で通すと、依存先が食い違っているのか
        // 材料が無いだけなのかを後段が区別できない
        let no_imports = r#"export function pad(value: number): string {
  return String(value);
}
"#;

        assert_eq!(
            ImportSet::from_tree(&tree_of(no_imports), Path::new("src/utils/pad.ts")),
            Err(ImportsUnavailable::NoDeclarations)
        );
    }
}
