//! チャンクのシグネチャに書かれた型名と、その位置。
//!
//! **解決前の綴りだけを持つ**（`rules/naming.md` の `type reference`）。その名前が何を
//! 指しているかを尋ねるのは `semantics` の担当で、ここが決めるのは**どこを指して
//! 尋ねればよいか**まで。`syntax` が LSP を知らない形は保つ
//! （`rules/architecture.md`「依存方向のルール」）。
//!
//! **解決できるのはソースに書かれた型名だけ。** 戻り値の注釈を省いた関数では、
//! hover の綴りに推論された型名が現れる（`function inferred(): Result`）のに、
//! **構文木のどこにもその名前が無い**。宣言を尋ねる `typeDefinition` は
//! **ソースの 1 点を指して**尋ねる問い合わせなので、書かれていない名前を指す位置を作れない。
//! この線は集める側を変えても動かない。倒れる向きは偽陰性で、その型名は綴りのまま比べられる。

use std::collections::BTreeSet;

use tree_sitter::Node;

use crate::source_position::SourcePosition;
use crate::syntax::tree::{source_position_of, transparent_wrappers_of, unwrapped_parent_of};

/// 型名 1 つを表すノードの種別。
///
/// キーワードの型（`string` / `number`）は grammar が `predefined_type` で表すので、
/// ここには入らない。**尋ねる相手が居ない名前を数えない**のが、問い合わせの数を
/// 抑える一番外側の網になる。
const TYPE_IDENTIFIER_KIND: &str = "type_identifier";

/// 修飾された型名を表すノードの種別（`money.Amount`）。
///
/// 綴りのうち末尾だけ（`Amount`）が [`TYPE_IDENTIFIER_KIND`] のノードになる。
const NESTED_TYPE_IDENTIFIER_KIND: &str = "nested_type_identifier";

/// 引数リストを載せるフィールド。
const PARAMETERS_FIELD: &str = "parameters";

/// 戻り値の型注釈を載せるフィールド。
const RETURN_TYPE_FIELD: &str = "return_type";

/// 型変数の宣言を載せるフィールド。
///
/// ノードの種別も同じ綴りなので、[`nodes_of_kind`] へもこれを渡す。
const TYPE_PARAMETERS_FIELD: &str = "type_parameters";

/// マップ型の束縛を表すノードの種別（`{ [K in "a"]: K }` の `K in "a"`）。
///
/// 型変数の宣言と同じく、**その綴りはこのシグネチャの中でだけ意味を持つ**。
const MAPPED_TYPE_CLAUSE_KIND: &str = "mapped_type_clause";

/// 条件型が型を捕まえる印を表すノードの種別（`infer U`）。
///
/// 捕まえた名前も、**その綴りはこのシグネチャの中でだけ意味を持つ**。
const INFER_TYPE_KIND: &str = "infer_type";

/// 名前を載せるフィールド。
const NAME_FIELD: &str = "name";

/// 型注釈を載せるフィールド。
const TYPE_FIELD: &str = "type";

/// シグネチャに書かれた型名 1 つ分と、その位置。**解決前**。
///
/// 綴りは書いた人の位置に依存する（輸入した `Amount` は、どのファイルの `Amount` かを
/// 綴りだけでは言えない）。**この型は尋ねる材料であって答えではない**
/// (`rules/naming.md`「`type reference` と `resolved type` を混ぜない」)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeReference {
    name: String,
    position: SourcePosition,
}

impl TypeReference {
    /// ソースに書かれた綴り。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// その綴りが置かれている位置。問い合わせを向ける先。
    pub fn position(&self) -> SourcePosition {
        self.position
    }
}

/// そのチャンクのシグネチャに書かれた型名。1 つも書かれていなければ空。
///
/// `nodes` はチャンクの型が書かれているノード（実装と、そのオーバーロード宣言）、
/// `source` はそれを含むファイル全体のソース。
///
/// **同じ綴りは 1 つにまとめる。** 尋ねる先は綴りごとに 1 箇所あればよく、
/// 同じ名前へ 2 度尋ねる理由が無い。**ノードをまたいでも 1 つにまとめる。**
/// 同じファイルの同じ綴りは同じ型を指すので、宣言ごとに尋ね直す理由が無い。
///
/// **型変数の束縛はノードごとに見る。** オーバーロード宣言はそれぞれが自分の型変数を
/// 宣言するので、まとめて 1 つの集合にすると**別の宣言の型変数が外側の型名を隠す**。
///
/// **空は「書かれていない」。** キーワードの型だけで書かれたシグネチャがこれで、
/// 集められなかったという状態は無い（構文木からは必ず採れる）ので `Option` にしない。
pub(super) fn type_references_of(nodes: &[Node<'_>], source: &str) -> Vec<TypeReference> {
    let mut references: Vec<TypeReference> = Vec::new();

    for node in nodes {
        // **束縛は、それを宣言した綴りの中だけに効かせる。** チャンクの型変数は包みまで
        // 届かず、包みの型変数もチャンクまで届かないので、同じ綴りが両側にあると
        // **外側の宣言を指している側まで落ちる**（落ちた型名は開かれず、別のファイルの
        // 同じ綴りと重なる = 偽陽性）
        for annotated in [annotated_nodes_of(*node), outer_annotated_nodes_of(*node)] {
            let declared = bound_type_names_of(&annotated, source);

            for node in annotated {
                for identifier in type_identifiers_of(node) {
                    let Some(reference) = type_reference_of(identifier, source) else {
                        continue;
                    };
                    if declared.contains(reference.name()) {
                        continue;
                    }
                    if references
                        .iter()
                        .any(|kept| kept.name() == reference.name())
                    {
                        continue;
                    }
                    references.push(reference);
                }
            }
        }
    }

    references
}

/// そのノード自身のシグネチャに、型注釈が書かれうるノード。
///
/// **ここが、そのノードの宣言した型変数が届く範囲**（[`bound_type_names_of`] が
/// 見る相手）。外側に書かれた注釈は [`outer_annotated_nodes_of`] が別に返す。
fn annotated_nodes_of(node: Node<'_>) -> Vec<Node<'_>> {
    let mut annotated = Vec::new();

    for field in [TYPE_PARAMETERS_FIELD, PARAMETERS_FIELD, RETURN_TYPE_FIELD] {
        if let Some(child) = node.child_by_field_name(field) {
            annotated.push(child);
        }
    }

    annotated
}

/// そのノードの外側に書かれていて、hover が答える綴りに現れる型注釈。
///
/// **自分の名前を持たないチャンクでは hover が代入先の名前を指す**（`chunk` の
/// `name_node_of`）ので、包みが言い切った型（`as` / `satisfies` / `<T>value` /
/// インスタンス化の型引数）と代入先の注釈（`const aliased: Handler` の `Handler`）も
/// シグネチャの一部になる。自分の名前を持つチャンクでは hover が関数自身の型を返すので空。
///
/// **名前を探す側と同じ包みを抜ける**（[`unwrapped_parent_of`]）。片方だけが抜けると、
/// `const f = (…) as (v: Input) => Input` の `Input` を集め損ねる。集め損ねた型名は
/// 開かれずに比較へ残るので、**別のファイルの同じ綴りの型が単一化可能に出る**（偽陽性）。
fn outer_annotated_nodes_of(node: Node<'_>) -> Vec<Node<'_>> {
    if node.child_by_field_name(NAME_FIELD).is_some() {
        return Vec::new();
    }

    let mut annotated = wrapper_types_of(node);

    let assigned =
        unwrapped_parent_of(node).and_then(|parent| parent.child_by_field_name(TYPE_FIELD));
    if let Some(assigned) = assigned {
        annotated.push(assigned);
    }

    annotated
}

/// そのノードを包んでいる式が書いている型。内側の包みのものから順に並ぶ。
///
/// `as` / `satisfies` が言い切った型・`<T>value` の型引数・インスタンス化に渡した型引数が
/// これで、**どれも hover が答える綴りに現れる**。
///
/// **包んでいる式から降りてきた子だけを外す。** 包みの種別ごとに型の載る場所を数え上げると、
/// フィールド名を持つもの（`instantiation_expression` の `type_arguments`）と持たないもの
/// （`as_expression`）で別々の引き方が要る。**残りの名前付きの子は型だけ**なので、
/// 降りてきた側を外せば種別を見ずに済む。
fn wrapper_types_of(node: Node<'_>) -> Vec<Node<'_>> {
    let mut types = Vec::new();
    let mut inner = node;

    for wrapper in transparent_wrappers_of(node) {
        let mut cursor = wrapper.walk();
        types.extend(
            wrapper
                .named_children(&mut cursor)
                .filter(|child| child.id() != inner.id()),
        );
        inner = wrapper;
    }

    types
}

/// その部分木にある型名のノードを、書かれた順に返す。
///
/// **修飾された型名は 1 つとして数える。** `money.Amount` を末尾の `Amount` として集めると、
/// 解決した綴りを差し込んだ先が `money.number` になる（差し込む側は綴り全体を
/// 1 つの型名として置き換える）。
fn type_identifiers_of(node: Node<'_>) -> Vec<Node<'_>> {
    let mut named: Vec<Node<'_>> = Vec::new();

    for found in nodes_of_kind(node, TYPE_IDENTIFIER_KIND)
        .into_iter()
        .chain(nodes_of_kind(node, NESTED_TYPE_IDENTIFIER_KIND))
    {
        if is_qualified_leaf(found) {
            continue;
        }
        named.push(found);
    }

    // 2 つの種別を別々に歩いたので、ソースに書かれた順へ戻す。
    named.sort_by_key(|found| found.start_byte());

    named
}

/// その部分木にある、指定した種別のノードを書かれた順に返す。
fn nodes_of_kind<'tree>(node: Node<'tree>, kind: &str) -> Vec<Node<'tree>> {
    let mut found = Vec::new();
    push_nodes_of_kind(node, kind, &mut found);

    found
}

/// 指定した種別のノードを、部分木を前順に歩いて書き足す。
///
/// 見つけても子へ降り続ける。総称型は名前と型引数が同じ部分木にいるので
/// （`Box<User>`）、そこで止めると `User` を数え落とす。
fn push_nodes_of_kind<'tree>(node: Node<'tree>, kind: &str, found: &mut Vec<Node<'tree>>) {
    if node.kind() == kind {
        found.push(node);
    }

    let mut cursor = node.walk();
    let children: Vec<Node<'tree>> = node.named_children(&mut cursor).collect();
    for child in children {
        push_nodes_of_kind(child, kind, found);
    }
}

/// そのノードが、修飾された型名の末尾か（`money.Amount` の `Amount`）。
///
/// 末尾は修飾ごと数えるので、単独では集めない。**尋ねる位置としては使う**
/// （[`asked_node_of`]）。
fn is_qualified_leaf(node: Node<'_>) -> bool {
    node.parent()
        .is_some_and(|parent| parent.kind() == NESTED_TYPE_IDENTIFIER_KIND)
}

/// その型名について尋ねるときに指すノード。
///
/// **修飾された型名では末尾を指す。** 先頭（`money.Amount` の `money`）は名前空間なので、
/// そこへ `typeDefinition` を送っても型の宣言は返らない。
fn asked_node_of(node: Node<'_>) -> Node<'_> {
    if node.kind() != NESTED_TYPE_IDENTIFIER_KIND {
        return node;
    }

    let mut cursor = node.walk();
    let tail = node
        .named_children(&mut cursor)
        .find(|child| child.kind() == TYPE_IDENTIFIER_KIND);

    tail.unwrap_or(node)
}

/// その注釈の中で束縛された型の名前。束縛が無ければ空。
///
/// `annotated` は 1 つの範囲に収まる注釈のノード（[`annotated_nodes_of`] か
/// [`outer_annotated_nodes_of`] のどちらか片方）。**渡された注釈そのものから採る。**
/// 別の範囲の注釈まで混ぜると、同じ綴りの束縛が**外側の宣言を指している側まで落とす**。
///
/// 束縛は 3 通りある。型変数の宣言（`<T>`）・マップ型の束縛（`[K in "a"]`）・
/// 条件型が捕まえる名前（`infer U`）。
/// **どれもその綴りはこのシグネチャの中でだけ意味を持つ**ので、解決の対象にしない。
///
/// **Why（束縛された名前を尋ねない）**: 束縛はチャンクごとに別のものなので、
/// 解決するとファイルごとに違う結果になる。`pickFirst<T>` と `head<U>` が
/// **単一化できなくなる**（正規化はこの 2 つを同じ形に直す前提で書かれている）。
///
/// **入れ子の束縛も見る。** `(a: T, cb: <T>(x: T) => T)` のように内側が外側の綴りを
/// 覆うと、外側だけを解決した差し込みが**内側の束縛まで書き換える**
/// （`<number>(x: number) => number` という綴りになる）。綴りが同じ名前は
/// まとめて尋ねない側に置く。
///
/// **Why not（束縛ごとに区別する）**: 区別するには型名を綴りではなく位置で持ち回る
/// ことになり、差し込みも位置で行う形へ変わる。倒れる向きは偽陰性（外側の
/// エイリアスが解決されないだけ）なので、綴りが壊れないほうを採った。
///
/// 制約に書かれた型名（`<T extends Amount>` の `Amount`、`[K in Keys]` の `Keys`）は
/// 束縛された名前ではないので、集める側に残る。
fn bound_type_names_of(annotated: &[Node<'_>], source: &str) -> BTreeSet<String> {
    let mut bound = BTreeSet::new();

    for annotated in annotated.iter().copied() {
        for declarations in nodes_of_kind(annotated, TYPE_PARAMETERS_FIELD) {
            let mut cursor = declarations.walk();
            let declared: Vec<Node<'_>> = declarations.named_children(&mut cursor).collect();
            for parameter in declared {
                push_name(
                    &mut bound,
                    parameter.child_by_field_name(NAME_FIELD),
                    source,
                );
            }
        }

        for clause in nodes_of_kind(annotated, MAPPED_TYPE_CLAUSE_KIND) {
            // 束縛されるのは先頭の型名だけ。`[K in Keys]` の `Keys` は制約なので残す。
            push_name(&mut bound, first_type_identifier_of(clause), source);
        }

        for inferred in nodes_of_kind(annotated, INFER_TYPE_KIND) {
            push_name(&mut bound, first_type_identifier_of(inferred), source);
        }
    }

    bound
}

/// その束縛が捕まえた型名のノード。捕まえていなければ `None`。
///
/// **先頭の 1 つだけが束縛。** `[K in Keys]` の `Keys` も `infer U extends X` の `X` も
/// 制約なので、集める側に残る。
fn first_type_identifier_of(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();

    node.named_children(&mut cursor)
        .next()
        .filter(|child| child.kind() == TYPE_IDENTIFIER_KIND)
}

/// そのノードが覆う綴りを、束縛された名前として書き足す。ノードが無ければ何もしない。
fn push_name(bound: &mut BTreeSet<String>, node: Option<Node<'_>>, source: &str) {
    let Some(name) = node.and_then(|node| source.get(node.byte_range())) else {
        return;
    };

    bound.insert(name.to_owned());
}

/// 型名のノード 1 つを、綴りと位置の組にする。
///
/// **綴りは書かれたまま、位置は尋ねる先。** 修飾された型名では 2 つが別のノードから来る
/// （[`asked_node_of`]）。
///
/// バイト範囲が文字の境界に乗っていなければ `None`。
fn type_reference_of(node: Node<'_>, source: &str) -> Option<TypeReference> {
    Some(TypeReference {
        name: source.get(node.byte_range())?.to_owned(),
        position: source_position_of(asked_node_of(node), source)?,
    })
}
