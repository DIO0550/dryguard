# 命名規約

`snake_case` / `CamelCase` などの綴りの流儀は **rustfmt と clippy が見る**ので、ここには書かない
（ツールが強制するものを規約に二重に書かない）。ここに書くのは、ツールが見られない部分だけ。

## 名前と実体を一致させる

**名前が約束することと、実際にやること・返すものを一致させる。**
名前を読んで想像した挙動と実装がずれていたら、名前が間違っている。

- **戻り値を名前に出す。** エラーの一覧を返すだけの関数を `validate_*` と名付けない
  （「検証して成否を返す」と読める）。返すものを名前に出して `chunk_errors_of`
- 探索していない関数に `find_*` を使わない
- **1 つのモジュール内で語彙を混在させない。** 内部だけ直して公開 API に古い名前を残さない

| NG | OK | 理由 |
|---|---|---|
| `validate_chunk(chunk)` | `chunk_errors_of(chunk)` | エラー一覧を返すだけで、成否は返さない |
| `find_domain(path)` | `domain_of(path)` | 何も探していない。パスから導いている |

## `And` を含む名前を作らない

`do_a_and_b` は、その関数が 2 つの振る舞いを持っている証拠。
**名前を工夫して押し込めるのではなく、処理を分ける。**
分けられないなら、2 つをまとめて表す 1 つの概念名を見つける。

## 条件を組んだ式に名前を付ける

`&&` / `||` で 2 つ以上の条件を組んだ式を `if` にそのまま置かない。
**何を判定しているのかを表す名前の変数に入れてから使う。**

```rust
// NG
if similarity > threshold && !domains_overlap && caller_domains.len() > 1 { ... }

// OK
let structurally_similar = similarity > threshold;
let domains_differ = !domains_overlap && caller_domains.len() > 1;
if structurally_similar && domains_differ { ... }
```

条件が 2 つ以上要る理由がコードから読み取れないなら、変数のそばに Why を残す。
単独の条件（`chunks.is_empty()` など）はそのまま置いてよい。

## 内容を表さない汎用語を使わない

`Info` / `Data` / `Detail` / `Manager` / `Helper` / `Util` は、何を指すのかを伝えない。

**名前が思いつかないのは、その型が 2 つの役割を抱き合わせているサイン**であることが多い。
改名で解決しようとする前に、役割を分けられないか確認する（分けた結果その型自体が不要になることもある）。

## このツールの語彙を固定する

**同じものをステージごとに違う名前で呼ばない。** `docs/dryguard-plan.md` が使っている語を出発点にする。

| 語 | 指すもの |
|---|---|
| `codebase` | スキャンの対象になるディレクトリツリー。`scan` が受け取る根の下 |
| `grammar` | ソースを読むのに使う tree-sitter の文法。拡張子で決まる（`.ts` / `.tsx`） |
| `chunk` | 比較の単位。関数・メソッド・impl ブロック |
| `pair` | 比較する 2 つの chunk |
| `candidate pair` | 構造類似度が閾値に届いた pair。`scan` が判定して出すのはこれだけ |
| `gram` | 構造類似度を測るときに突き合わせる、正規化トークンの並び 1 つ分 |
| `signal` | 判定の材料。構造類似度・型シグネチャ・呼び出し先 / 呼び出し元・モジュール距離 |
| `verdict` | 判定の結果（`EXTRACT-CANDIDATE` / `DO-NOT-EXTRACT` / `REVIEW`） |
| `reason` | 判定を傾けた根拠 1 件。シグナルの値と、それが傾けた向きの組 |
| `lean` | シグナルが判定を傾けた向き（共通化する側 / しない側 / どちらでもない） |
| `domain` | ドメイン。ディレクトリ構造からの推定と `dryguard.toml` の宣言で決まる |
| `import` | 依存の宣言。ソースに書かれた `import` / `export ... from` そのもの |
| `specifier` | `from` の後ろに書かれた文字列（`"./pad"`）。**解決前** |
| `module path` | 指定子を importer の位置から解決した依存先（`src/utils/pad`）。**解決後** |
| `module distance` | 2 つのファイルを隔てているディレクトリの段数 |
| `frame` | LSP のストリーム上の 1 通分。`Content-Length` ヘッダと、それが数えた本文 |
| `payload` | frame の本文。JSON-RPC のメッセージ 1 通そのもの |
| `handshake` | `initialize` 要求 → 応答 → `initialized` 通知。ここまでで 1 つ |
| `session` | handshake を終えた接続。問い合わせを送れる状態。握手前は `client` |
| `workspace root` | `initialize` でサーバに見せるディレクトリ。開かせるファイル群の共通の祖先 |
| `document` | サーバに開かせるソースファイル 1 つ分。URI・`language id`・中身の組 |
| `language id` | LSP がサーバに伝える言語の名前（`typescript` / `typescriptreact`） |
| `hover` | ソースの 1 点を指して、そこにある名前の型を尋ねる問い合わせ |
| `type definition` | ソースの 1 点を指して、そこに書かれた型がどこで宣言されているかを尋ねる問い合わせ |
| `declaration site` | `type definition` が返す宣言の場所。ファイルと、その中の 1 点 |
| `references` | ソースの 1 点を指して、そこにある名前を使っているところを尋ねる問い合わせ |
| `reference` | `references` が返す 1 件。**その名前を使っている側**のファイルの位置 |
| `caller domain` | 参照元が属する `domain`。ドメインごとの件数を持つ |
| `progress` | サーバが自分で始めた作業（プロジェクトの読み込みなど）。作成の要求と、終わりの通知で挟まれる |
| `source position` | ファイルの中の 1 点。行と、**UTF-16 のコード単位で数えた**列 |
| `signature text` | hover が返した型の綴りそのもの。**正規化前** |
| `type spelling` | 型 1 つ分の綴り。`signature text` を割った先の 1 つ（引数の型・戻り値の型・制約） |
| `type reference` | チャンクのシグネチャに書かれた型名 1 つ分と、その位置。**解決前** |
| `resolved type` | その型名が指していた型の綴り。**解決後** |
| `type signature` | 引数名を落とし、型変数を出現順に付け替えた形。**正規化後**。比較はこれで行う |
| `unifiable` | 2 つの `type signature` が同じ型構造に重なること（単一化可能） |

`snippet` / `fragment` / `candidate`（chunk の意味で）/ `label`（verdict の意味で）は使わない。
**`candidate` が指すのはペアであって chunk ではない。**

**`reason` は文ではなく構造。** 「依存先ドメイン不一致」のような人が読む文は
`reason` を出力側が組み立てた結果で、`reason` そのものではない。文にしてから持つと、
**判定に効いた値と向きが文字列に埋もれて後段が読めない**（`--explain` が
シグナルごとの効き方を出せなくなる）。

**`specifier` と `module path` を混ぜない。** どちらも文字列だが、解決前は
書いた人の位置に依存し、解決後は依存しない。同じ依存先が別の綴りで書かれるので、
**解決前のまま比べると共有している依存を「別物」と数える**（`ModulePath` を
newtype にしているのはこのため）。

**`frame` と `payload` を混ぜない。** 区切りを付ける側（`lsp::framing`）と中身を読む側
（`lsp::message`）はモジュールが別で、**失敗の直し先も別**（`Content-Length` が壊れているのと、
JSON が壊れているのは違う話）。1 語で呼ぶと、どちらの層で落ちたのかがエラーの名前から消える。

**`signature text` と `type signature` を混ぜない。** どちらも 1 つの関数の型を指すが、
綴りは書いた人の付けた引数名と型変数名に依存し、正規化後は依存しない。**綴りのまま比べると、
引数名が違うだけのペアが別物になる**（`specifier` と `module path` を分けているのと同じ形）。
サーバが返す綴りには宣言形（`function decl(a: string): number`）と値形
（`const arrow: (a: string) => number`）があり、**同じ型でも書かれ方が 2 通りある**。
綴り側を `SignatureText` の newtype にしているのはこのため（`ModulePath` と同じ形）。

**`signature text` と `type spelling` を混ぜない。** どちらも綴りだが、`signature text` は
1 つの関数の型全体で、接頭辞（`(method)` / `constructor`）が付くことがあり**型としては読めない**。
`type spelling` は型 1 つ分なので、**型として構文解析できる**。差し込みが型名の位置を
構文木で決められるのは後者だけ（`syntax::type_spelling`）。

**`type reference` と `resolved type` を混ぜない。** どちらも型を指す綴りだが、
書かれた型名は**書いた人の位置に依存する**（輸入した `Amount` は、どのファイルの `Amount` かを
綴りだけでは言えない）。**綴りのまま比べると、同じ型を指す `Amount` と `number` が別物になる**
（`specifier` と `module path` を分けているのと同じ形）。集めるのは `syntax`、
解決するのは `semantics` と、持ち場も分かれる。

**`module distance` と `caller domain` を混ぜない。** どちらもディレクトリで測るが、
前者は**そのチャンク自身がどこに置かれているか**、後者は**実際に誰が使っているか**。
片方が `utils/` に置かれていても、呼び出し元が 1 つのドメインに偏っていれば
「そのドメインのもの」と言える。**置き場所の代理指標を、使われ方の観測で置き換えない**
（重ねる。`docs/dryguard-plan.md`「Phase 0 のディレクトリ距離との関係」）。

**`grammar` と `language id` を混ぜない。** どちらも拡張子で決まるが、`grammar` は
tree-sitter がソースを読むための文法、`language id` は LSP サーバに言語を伝える綴りで、
**綴りを決めているのが別の相手**（前者は tree-sitter のクレート、後者は LSP）。
拡張子の一覧そのものは `Grammar` が 1 箇所で持ち、`language id` はそこから導く。

**語を増やすときは、既にある語で言えないかを先に確かめる。**
新しい語を足したら、この表にも足す。

## `create` / `new` / `from_*` の使い分け

- **`new`**: 引数がその値の材料であるとき（`Location::new(path, line)`）
- **`from_*`**: 別の表現から作り直すとき。変換元を名前に出す（`Chunk::from_node`）
- 判定は `is_*` / `has_*`、変換は `to_*` / `into_*`（Rust の慣習に従う）
- **複数をまとめて返すものは `<返すもの>_of`**（`chunk_errors_of` / `tokens_of`）

## `collect` を名前に使わない

**自前の関数・型の名前に `collect` / `Collection` を入れない。** 代わりに
`<返すもの>_of` で、**返すものを名前に出す**（`Iterator::collect` の呼び出しは対象外）。

**Why**: Rust では `collect` が `Iterator::collect`（イテレータを別のコレクションへ集約する）と
強く結びついている。自前の関数に付けると、読む側が標準のそれと同じ操作を想像する。

**Why not（「集める」関数には使ってよい、としない）**: 「集めている」かどうかは
**呼ぶ側から見えない実装の手順**で、名前が伝えるべき「何が返るか」を押しのける。
実際 `collect_chunks` は 2 箇所から集めていたが、呼ぶ側が知りたいのは
**チャンクの組が返ること**だった。
