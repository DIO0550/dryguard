# dryguard — 偶発的重複の判定CLI 計画案 v2

## 何を作るか

AIコーディングエージェントは、テキスト的に似たコードを見つけると安易に共通化する。しかし「テキストが似ている」と「変更理由が同じ」は別物で、偶発的な重複 (incidental duplication) を共通化すると間違った抽象化が生まれる。

このツールは、似たコードペアを検出するだけでなく、**型・依存・参照の意味情報から「共通化してよいか / すべきでないか」を理由付きで分類する**。

分類は3種類:

| ラベル | 意味 |
|---|---|
| `EXTRACT-CANDIDATE` | 構造・型・依存ドメインが一致。共通化してよい |
| `DO-NOT-EXTRACT` | 構造は似ているが依存先・呼び出し元のドメインが別。偶発的重複 |
| `REVIEW` | 中間ケース。人間の判断が要る |

出力は text と JSON の両対応。JSONはAIエージェントに読ませて自己修正させる用途を想定する。

対象言語はまず TypeScript、次に Rust。LSPサーバを差し替えれば他言語も足せる設計にしておく（Python / Java / Swift は「Phase 6: 他言語対応」）。

---

## 差別化ポイント

類似コード検出そのものは珍しくない。このツールの価値は以下の3点にあり、いずれも自前で全ステージを持たないと成立しない。

1. **「共通化するな」を言える**。検出だけのツールは重複を報告するだけで、共通化の是非は判断しない。
2. **判断に理由が付く**。ML・埋め込みを使わずルールベースにするのは、説明可能性がこのツールの価値そのものだから。
3. **AIエージェント向けの出力**。人間のリファクタリング支援ではなく、エージェントの自己修正ループに組み込むJSONが一級市民。

Stage 1 を外部ツールに委譲せず自前で持つ理由:

- 検出だけのツールが返すのは類似スコアと `file:line` 程度で、Stage 2 のLSP問い合わせに必要な構造情報は結局自前で取り直しになる
- Stage 3 の判定に効く正規化の粒度を、Stage 1 側で調整できなくなる
- 単一バイナリ配布と dogfooding (Rust製ツールでRustを解析する) が崩れる

---

## アーキテクチャ

3ステージのパイプライン。

```
[Stage 1: 候補抽出]      [Stage 2: 意味情報収集]   [Stage 3: 分類]
 tree-sitter              LSP                      ルールエンジン
  ├ 関数/メソッド単位で    ├ hover (型シグネチャ)     ├ シグナル統合
  │ チャンク化            ├ callHierarchy           ├ ドメイン距離判定
  ├ AST正規化             ├ references              └ 理由付きラベル
  └ 類似ペア列挙          └ documentSymbol
```

### Stage 1: 候補抽出 (LSP不要・高速)

- tree-sitter で関数・メソッド・implブロック単位に分割
- ASTを正規化 (識別子→プレースホルダ、リテラル→型タグ) して Type-2 相当のクローン検出
- 正規化トークン列の類似度で候補ペアを列挙
- 正規化トークンが下限 (`syntax::token` の `MINIMUM_TOKEN_COUNT`) に届かない関数は `scan` の候補列挙に入れない。1〜3 行のアクセサ・委譲は形が 1 つしかなく、どれとも構造類似度 1.0 で並ぶため (Issue #271)。`compare` は名指しされれば比べる
- **同時に import / use 文を収集** し、各チャンクの依存モジュール集合を持たせる (Stage 3 の初期シグナル源)
- 正規化で捨てた葉の綴りも位置ごとに残し、**正規化トークン列が一致するペアに限って**同じ位置どうしを突き合わせる。同じ形のまま別の相手へ委譲する関数（Issue #289）を、構造類似度 1.0 の中から分ける

n² 比較になるため、規模が問題になった時点で MinHash/LSH またはブルームフィルタによる事前フィルタを入れる。**最初から入れない。**

チャンクの単位は**関数・メソッドで確定**。ブロック単位までは落とさない (Issue #18)。自前コーパス (47関数 / 1081ペア) を測ると、ブロック単位にして初めて閾値を超える関数ペアは9件で、内訳はガード節・早期return・フィルタループのcontinueという言語のイディオムだけだった。得るものがイディオムである一方、比較の単位は47→87、ペアは1081→3741に増える。**判定を考えずに書かれた実コードで偽陰性が出たら再検討する** — 計測は `harness/phase0/chunk-unit-probe/`、対象の入手は Issue #88。

### Stage 2: 意味情報収集 (LSP)

候補ペアに対してだけ問い合わせる。全体に投げると rust-analyzer が実用にならない。

- **型シグネチャの正規化比較**: hover から取得し、引数名を捨てて型構造で比較。ジェネリクスは単一化を試みる
- **コールグラフ比較**: callHierarchy/outgoingCalls で呼び出し先集合を取り、依存モジュール・型の重なりを測る
- **参照元比較**: references で呼び出し元を取り、それが属するモジュール/ディレクトリ (= ドメイン) を比較。Rust はテスト属性の関数や単純な `cfg(test)` の範囲内の参照を除く（`rules/naming.md` の `reference`）。候補を含める `--include-tests` とは独立。参照元を読み取れないときは部分集計せず、理由付きで測れないとする
- Rust は直接囲む impl の対象型・型変数・境界をメソッドの比較に使う。Self とレシーバは対象型へ置換し、型変数は impl / メソッドの束縛元を区別して改名する。直接囲む trait impl に定義された Self::Item と完全修飾形は trait の宣言元を確認して RHS を展開し、impl 型変数と GAT の型引数を代入する。inherent impl・別 trait の impl の中の完全修飾形は implementation で trait の impl を集め、境界を持たず、型変数に使用側の型変数か具体的な型を当てはめるだけで対象型と trait 引数が一致する 1 つ（境界の無い blanket impl を含む）を選んで展開する。関連型 RHS の Self は使用側の impl の対象型を代入する。関連型 RHS の中の投影は RHS を書いた impl を使用側として再帰して開き、入れ子の投影（`<<Self as T>::Item as O>::Out`）は 1 段目が具体的な型に閉じたときだけ型パラメータを持たない 2 段目の impl を選ぶ。trait impl の中の短縮形（Self::Item）が supertrait の関連型を指すなら、supertrait の閉包で definition の所有 trait 以外にその名前を宣言する trait が無いときに、所有 trait の impl を同じ規則で選ぶ。型引数を書いた supertrait・境界を持つ impl の選択と const generics は未対応（inherent impl・無関係な trait の impl の中の短縮形と短縮形の入れ子は rustc が拒否する）
- Rust はトレイト境界も考慮。TS / Rust とも型エイリアスを展開する。Rust は引数のないエイリアスの右辺の型名を宣言側ソースから辿り、再輸出・エイリアス連鎖を宣言元の同一性で比較する。型引数と既定の型引数も当てはめるが、ジェネリック宣言の右辺と既定値はプリミティブや束縛された型パラメータに閉じるものに限る。ライフタイム・const 引数、未知の型構文、循環・展開上限は理由付きで測れないとする。直接の配列長と型引数のないエイリアス RHS の配列長は、宣言側の整数・usize 定数の連鎖・`+ - * / %` を評価する。hover の `{const}` はソースとの型構造の対応を確認してから復元する。中間値も65535以下に限り、式・連鎖・評価量に上限を設ける。未対応・循環・上限・対応不能は理由付きで測れないとする（詳細は `rules/architecture.md`）

### Stage 3: 分類

シグナルを統合してルールベース (閾値 + 決定木) で判定。

| シグナル | 共通化候補に傾く | 非推奨に傾く |
|---|---|---|
| 型シグネチャ | 単一化可能 | 単一化不能 / 別ドメインの型 |
| 呼び出し先 | 同じユーティリティ/型に依存 | 依存先モジュールが別ドメイン |
| 葉の綴り（同じ位置の名前・リテラル） | — （違いが無くても傾けない） | 正規化トークン列が揃うのに、束縛した名前の付け替えでない差し替えがある。同じドメインでも `REVIEW` に留める（Issue #289） |
| 呼び出し元 | 同一レイヤー・同一機能 | 別機能・別レイヤーに分布 |
| モジュール距離 | 近い | 遠い (billing/ vs inventory/ など) |

ドメイン境界の自動推定は難しいので、ディレクトリ構造ベース + `dryguard.toml` での手動宣言を併用する。

宣言はパスの glob で、ファイル単位まで書く（Issue #36 で決めた。実装は Issue #237）。

```toml
[domains]
billing = ["src/billing/**", "src/**/invoice*.ts"]
inventory = ["src/inventory/**", "src/**/product*.ts"]
```

- **宣言が推定に勝つ。** 宣言に当たったファイルは、ディレクトリから出していたドメインを宣言の名前で置き換える（モジュール距離・呼び出し元ドメイン・呼び出し先ドメインの 3 つ。呼び出し先は #36 の後に判定へ入ったので #237 で足した）。呼び出し元・呼び出し先の**観測**そのものは宣言で上書きしない
- **1 つのファイルが名前の違う 2 つの宣言に当たったら、どちらかを選ばない。** 比べる 2 ファイル（`scan` なら走査対象すべて）は判定の前に止め、参照元・呼び出し先はそのシグナルを「測れない」にする（サーバが答えるまで分からず、止めると出せていた候補ペアまで捨てる）
- **宣言に当たらないファイルは推定のまま。** 宣言が無ければ今と同じ判定になる
- 関数単位の宣言は持たない。1 つのファイルに別の機能が同居する形は、呼び出し元の観測で分かれる

層で分けたレイアウト（`controllers/` `services/` `repositories/` の下に複数の機能が並ぶ）では、ディレクトリからの推定が別の機能を同じドメインに畳む。自前の置き方のコーパス（`tests/layouts/`）では、候補 23 ペアのうち 16 ペアで、正解のドメインで出し直すとラベルが変わった。ただしこれは外れ方の証拠であって頻度の証拠ではなく、頻度は実コードで測る（Issue #88）。

---

## 作る順番 — 縦切り (walking skeleton)

各ステージを順に完成させる横切りではなく、**全ステージを雑に貫通させてから厚くする**。理由は、このツールの価値がすべて Stage 3 にあるため。Stage 1 を作り込んでから Stage 2 に進むと、価値のある部分に到達する前に既存ツールの再実装で消耗する。

### Phase 0: 貫通させる (LSPなし)

目的は動くツールを作ることではなく、**仮説を検証すること**。

> 実際のコードベースに「構造は似ているが依存ドメインが違うペア」は、意味のある頻度で存在するのか？

- 対象は TS のみ / コマンドは `compare <locA> <locB>` のみ / 出力は text のみ
- Stage 1 は Jaccard の素朴な実装。精度は求めない
- Stage 2 は**飛ばす**。ドメインシグナルは import 文 + ディレクトリ距離のみ
- Stage 3 はハードコードの閾値で3ラベルを出す

これを自分のリポジトリ (`pdfmod`, `coding-task-lab` など) に当てて、`DO-NOT-EXTRACT` が実際に出るか見る。出ないなら LSP を書く前に方針を見直せる。

### Phase 1: Stage 1 を厚くする

- tree-sitter に移行、AST正規化を入れる
- `scan` コマンド (コードベース全体)
- 規模が問題になったら事前フィルタ

### Phase 2: Stage 2 を入れる (TS)

- typescript-language-server を子プロセスで起動、JSON-RPC クライアント自作
- hover / references を Stage 3 のシグナルに追加
- ここで初めて「型シグネチャ単一化可能」判定が入り、`EXTRACT-CANDIDATE` 側の精度が上がる

### Phase 3: Stage 3 を厚くする

- callHierarchy 統合
- 判定ルールを整理し、`--explain` でシグナル値を全表示
- 閾値の設定ファイル外出しは**この段階で**。Phase 0-2 はハードコードで、実際に調整したくなった項目だけ切り出す

### Phase 4: Rust 対応

- rust-analyzer。起動が重いのでキャッシュ戦略は使ってみてから考える
- ここから dogfooding が効く

### Phase 5: エージェント連携

- `check --diff` (git diff範囲のみ)
- JSONスキーマ整備、pre-commit フック化

### Phase 6: 他言語対応

- Python / Java / Swift を、TypeScript・Rust と同じ 3 ステージ（候補抽出 → 意味情報収集 → 分類）で扱う (Issue #360)
- 「LSPサーバを差し替えれば他言語も足せる設計にしておく」（「何を作るか」）を、実際に 3 言語足して確かめる回でもある
- 参照元のテスト範囲の除外（Rust の `syntax::rust_test_scope` に当たるもの）は入れない。各言語の実コーパスで、テスト内の参照が呼び出し元ドメインを歪めるかを測ってから起こす

進め方は 2 段。

1. **準備（言語に依存しない）**: 「Rust か、そうでなければ TypeScript」の 2 択で書かれた分岐を、網羅 `match` か grammar ごとに引く並びに直す。3 つ目の grammar を足したとき、2 択のままだと**コンパイルエラーにならずに TypeScript 側へ倒れる**。先に直すと、言語ごとの作業は「variant を足して、コンパイラが指す場所を埋める」形になる
2. **言語ごと**: Phase 4（Rust、Issue #37 → #41）と同じ順で縦に通す。Stage 1（grammar・チャンク・正規化・import・葉の綴り）→ 走査と CLI → Stage 2（サーバ・CI・型シグネチャ）→ 実コーパスで測る。言語の順番は決めない（3 言語は互いに独立で、準備だけが共通の前提）

#### 言語を足すときに触る場所

言語ごとの作業の順（Stage 1 の `syntax` → 走査と CLI → Stage 2 の `lsp` / `semantics` / `pipeline` → CI）に、モジュールの責務（`rules/architecture.md`「3 ステージのパイプライン」）ごとにまとめて並べる。**行番号ではなく関数名・定数名で指す** — 準備の Issue が入るたびに行も形も変わるため。「今の形」の欄は Issue #360 の時点（`main` 6b99873）のもので、準備が済んだ行から古くなる。

| モジュール | 場所 | 言語ごとに足すもの | 今の形 |
|---|---|---|---|
| `syntax::tree` | `Grammar`・`Grammar::of_path`・`Grammar::language`、`Cargo.toml`（grammar クレートは `harness/deps/resolve.sh add` で足す） | variant・拡張子・tree-sitter の言語 | `language` は網羅 `match`、`of_path` は拡張子の `match`（漏れても読めないだけ） |
| `syntax::tree` | `Grammar::same_language_as` | 同じ言語として比べる grammar の組 | **2 択**（`matches!`。足さないと新しい言語のチャンク同士が比べられない） |
| `syntax::tree` | `TRANSPARENT_EXPRESSION_KINDS` | 値を通すだけの包みの種別 | 全 grammar に当たる TypeScript の表 |
| `syntax::chunk` | `is_chunk_node`・`CHUNK_KINDS` | チャンクにするノードの種別 | **2 択**（Rust でなければ TypeScript の表） |
| `syntax::chunk` | `Chunk::from_node` の `annotated` / `overload_declarations` / `type_references`、`OVERLOAD_DECLARATION_KINDS`・`PARAMETER_KINDS`・`TYPE_ASSERTION_KINDS` | 注釈を省ける位置・オーバーロード宣言・型名の集め方 | **2 択が 3 箇所**（Rust でなければ TypeScript の表） |
| `syntax::chunk` | `Chunk::from_node` の `rust_impl_header` | 囲む宣言のヘッダーを運ぶか | 網羅 `match` |
| `syntax::chunk` | `is_test_function` | `test function` の印 | 網羅 `match` |
| `syntax::token` | `NAME_KINDS`・`LITERAL_KINDS`・`COMMENT_KIND` | 名前・リテラル・コメントのノード種別 | grammar を引数に取らない 1 つの表。**種別名が文法をまたいで別の意味にならないか**を確かめて足す |
| `syntax::import` | `ImportSet::from_tree`、`DEPENDENCY_STATEMENT_KINDS`・`IMPORT_EXPORT_KINDS`・`TYPE_ONLY_KINDS` ほか TypeScript の種別の表 | 依存の宣言の読み方と、`module path` への解決 | **2 択**（Rust でなければ TypeScript） |
| `syntax::leaf_divergence` | `signature_type_nodes_of` の引数の種別・`bound_names_of` の束縛の集め方 | 引数の種別・束縛の形 | 網羅 `match` が 2 箇所 |
| `syntax::leaf_divergence` | `BINDABLE_LEAF_KINDS`・`OUTSIDE_NAME_PARENT_KINDS` | 束縛として扱う葉・外の名前を指す親の種別 | 全 grammar に当たる表 |
| `codebase` | `SourceLanguage`・`SourceLanguage::accepts` | variant と、受け付ける grammar | **2 択**（Rust でなければ TypeScript） |
| `codebase` | `EXCLUDED_DIRECTORY_NAMES` | 降りない生成物・依存の置き場（Python の `.venv` など） | 全言語共通の表 |
| `cli` | `LanguageOption` | `--lang` の値 | enum |
| 入口（`main.rs`） | `source_language_of` | `--lang` から `SourceLanguage` へ | 網羅 `match` |
| 入口（`main.rs`） | `report_compare` | `--lang` とファイルの言語の突き合わせ | **2 択が 2 つ**（`is_rust`） |
| 入口（`main.rs`） | `language_servers`（`report_scan` / `report_check` / `report_compare` が使うサーバの並び） | 言語ごとのサーバ | TypeScript と Rust の 2 本を固定で渡す |
| `lsp` | `ServerLanguage`・`ServerCommand`（`*_SERVER`・`*_PROJECT_MARKERS`・`ServerCommand::rust` に当たる組み立て） | 起動方法・`project marker`・言語 | variant は enum。`ServerCommand::new` は独自サーバを TypeScript として扱う |
| `lsp` | `ServerCommand::supports` | そのサーバが尋ねられる grammar | `ServerLanguage` × `Grammar` の網羅 `match`（Issue #363） |
| `lsp` | `ServerLanguage::hover_fences` | hover のどのコードフェンスに綴りがあるか（試す順） | 網羅 `match`。`lsp::hover` は値を受け取るだけ（Issue #367） |
| `lsp` | `ServerLanguage::settled_attempts` | 読み込み中の答えを捨てて尋ね直す回数 | 網羅 `match`。`lsp::connection` は値を受け取るだけ（Issue #367） |
| `lsp` | `ServerLanguage::asks_project_membership` | `project membership` を尋ねるか | 網羅 `match`。`Session::needs_project_membership_query` はこれを返すだけ（Issue #367） |
| `lsp::document` | `LanguageId`・`LanguageId::from_grammar` | `language id` の綴り | 網羅 `match` |
| `semantics::type_signature` | `type_signature_outcome_of` / `rust_type_signature_outcome_of` の系統 | hover の綴りから型シグネチャを組む系統 | TypeScript と Rust の 2 系統 |
| `syntax::type_spelling` / `syntax::type_structure`（Rust は `syntax::rust_callable`） | 綴りを構文解析する箇所 | hover の綴りの読み取り | `Grammar::TypeScript` 決め打ち |
| `pipeline` | `resolved_type_signature_outcome_of` | 型シグネチャの系統の振り分け | **2 択**（Rust でなければ TypeScript） |
| `pipeline` | `LanguageServers`・`ServedSemantics`（`scan_of_chunks` が grammar からサーバを引き、チャンクへ意味情報を配る） | 言語ごとのサーバと意味情報 | **2 本固定**。配り方は Rust でなければ TypeScript の意味情報を当てる |
| CI | `.github/workflows/rust.yml` の「LSP サーバ」「rust-analyzer」の段 | サーバの導入。npm 配布なら「LSP サーバ」の段に倣う（`harness/deps/resolve-node.sh` と `pnpm-lock.yaml`）。それ以外は「rust-analyzer」の段に倣い、版を固定して取り sha256 で確かめる | 「LSP サーバ」が `pnpm install --frozen-lockfile` で typescript-language-server、「rust-analyzer」が curl と `sha256sum --check` で rust-analyzer |
| テスト | 実サーバのテスト（TypeScript は `tests/semantics.rs`、Rust は `tests/rust_lsp.rs`）・`tests/language_cli.rs`・`tests/fixtures/` | 実サーバのテスト（`#[ignore]`、CI の `cargo test -- --ignored` が走らせる）・CLI の言語の振り分け・フィクスチャ | 言語ごと |

**触らなくてよいもの**: `semantics::caller_domain` の `ReferenceSources::is_test` は Rust 以外の参照を除かないので、新しい言語は TypeScript と同じく全部数える（上のスコープ外どおり）。テスト参照の除外を足すときの置き場所はここ。`codebase::typescript_paths_of` と `pipeline::scan_of` は名前どおり TypeScript 専用の入口で、**`SourceLanguage::accepts` を網羅 `match` に直した後なら**言語を足しても黙って倒れない（直す前は `grammar != Grammar::Rust` なので、新しい言語のファイルまで拾う）。

**コード以外で言語を並べている場所**も同じ回に直す: `README.md`（`--lang`・`--include-tests`・拡張子・サーバ）、`AGENTS.md`「プロジェクト構成」（外部依存・LSP サーバ）、この計画書の「CLI 仕様 (案)」と「技術スタック」、`rules/naming.md` の語彙表（`grammar`・`test function`・`project marker`・`language id`）。

---

## CLI仕様 (案)

```
dryguard compare <locA> <locB>   # 特定の2関数を比較 (file:line)   [Phase 0]
dryguard scan [path]             # コードベース全体をスキャン       [Phase 1]
dryguard check --diff [rev]      # git diff範囲のみ (フック用)      [Phase 5]
dryguard check --diff --staged   # ステージ済みの変更だけ (pre-commit 用) [Phase 5]

オプション:
  --lang ts|rust|auto
  --format text|json
  --threshold <0-1>
  --fail-on do-not-extract        # 非推奨ペアがあればexit 1
  --explain                      # 判定根拠のシグナル値を全表示
  --include-tests                # scan / check のみ。Rust / TypeScript のテスト関数も比較する (既定は外す)
```

`--lang` の値は言語を 1 つ足すたびに増える（Phase 6 で 3 言語分。綴りは言語ごとの Issue で決める）。値は enum の `LanguageOption` が持ち、**選択肢と、それが変える振る舞いは同じ差分に入れる**（`rules/coding.md`「値の語彙を型で閉じる」）。`--include-tests` が外す `test function` の印も、言語ごとに決めて足す。

出力イメージ:

```
[DO-NOT-EXTRACT] src/billing/discount.ts:42 <-> src/inventory/reorder.ts:18
  構造類似度: 0.91
  理由: 依存先ドメイン不一致 (billing::Invoice vs inventory::Stock)
        呼び出し元も別機能に分布 (billing 3件 / inventory 5件)
  提案: 偶発的な重複の可能性が高い。共通化せず分離を維持。

[EXTRACT-CANDIDATE] src/utils/formatDate.ts:10 <-> src/report/dateHelper.ts:5
  構造類似度: 0.95 / 型シグネチャ: 単一化可能
  理由: 依存先・参照元パターンが一致
```

---

## 技術スタック

実装言語は **Rust**。

- tree-sitter のネイティブバインディングが最もまとも
- `lsp-types` + 自前 JSON-RPC クライアント (子プロセスで LSP サーバ起動)
- 単一バイナリで配布が楽、Rust解析対象なので dogfooding できる
- その他: `rayon` (Stage 1 並列化), `serde`, `clap`

LSPサーバ: TS は typescript-language-server、Rust は rust-analyzer。Phase 6 で Python は pyright、Java は Eclipse JDT Language Server（jdtls）、Swift は sourcekit-lsp を足す（配布形は言語ごとの親 Issue #368 / #369 / #370。hover の綴りの形・references / callHierarchy の対応・読み込みの待ち時間は未実測で、各言語の「実サーバで確かめる」Issue が最初に測る）。

---

## リスクと対処

| リスク | 対処 |
|---|---|
| そもそも偶発的重複が見つからない | Phase 0 で最初に検証する。ここで潰れるなら安いうちに潰す |
| rust-analyzer の起動・インデックスが遅い | 候補確定後にまとめて問い合わせる。それでも遅ければデーモンモード検討 |
| callHierarchy がサーバによって不安定 | references ベースのフォールバックを用意 |
| LSPサーバが黙り込むとCLIが止まる | 何も送ってこない時間（沈黙）が 120 秒を超えたら止め、`exit` の後 10 秒で終わらなければ kill する (Issue #108)。合計時間ではなく沈黙で切るのは、rust-analyzer が読み込みの間ずっと進捗を流すため（実測の最長の沈黙 6.69 秒に対して、落ち着くまでの合計は 6.6〜24.4 秒と揺れる）。合計で切ると正常に遅いサーバを殺す。ただし通知・サーバの要求だけを流し続けて答えないサーバは沈黙にならないので、要求 1 本の応答・始まった作業の終わりを待つ合計に 30 分の安全網を置く（1 回の待ちの実測の最長は dryguard 自身で 15.2 秒、tokio 1.40.0 で 29.03 秒。Issue #319）。1 フレームを少しずつ送り続けるサーバも沈黙にならないので、この安全網はフレームの途中で続きを受け取った後にも確かめる（Issue #347）。書き込みも、1 塊（8 KiB）が 120 秒受け取られなければ止める（stdin を読まないサーバにパイプのバッファを超えるドキュメントを開かせると、書き込みで止まるため。Issue #319） |
| 偽陽性 | 判定は常に理由付きにし、閾値は設定で調整可能に。最初から精度を求めず自分のリポジトリで育てる |
| n² 比較のコスト | 問題が出るまで最適化しない。出たら MinHash/LSH かブルームフィルタ |

---

## 未決事項

- **`--dry-run` は使わない**: 名前と衝突して誤解を生むため、確認系オプションが要る場合は別の語を当てる
