# `--format json` の読み方

`--format json` は、**エージェントが読んで自己修正ループに組み込む**ための出力
（`docs/dryguard-plan.md`「差別化ポイント」）。形は
[`json-output.schema.json`](json-output.schema.json)（JSON Schema draft 2020-12）が持つ。
この文書は**どのフィールドを見て、何をすべきか**を持つ。

## 出力の形

| コマンド | 最上位 | スキーマの定義 |
|---|---|---|
| `compare` | ペア 1 つの判定 + `schema_version` | `compare_output` |
| `scan` / `check` | `schema_version`・`candidate_pairs`・`skipped_files`・`unchunkable`・`walked` | `scan_output` |

`scan` / `check` の `candidate_pairs` の要素は `compare` の出力と同じ形のペア
（`schema_version` は最上位に 1 つだけで、ペアには付かない）。

## 読む順

1. **`schema_version`** — 知らない版なら**読まずに止まる**（下の「版と破壊的変更」）
2. **`verdict`** — 取る行動はこれだけで決める（下の表）
3. **`reasons[]`** — なぜその判定になったか。`signal` ごとに `lean`（傾けた向き）と `value` を持つ
4. **`reasons[].value.status`** — `measured` なら値が、`unmeasurable` / `not-asked` なら `reason` が並ぶ

`suggestion` は人が読む日本語の文で、**契約に入らない**（文言は版を上げずに変わる）。
分岐は `verdict` で行い、`suggestion` を解釈しない。

## 判定ごとに取る行動

| `verdict` | 意味 | エージェントが取る行動 |
|---|---|---|
| `EXTRACT-CANDIDATE` | 共通化してよい | 1 つにまとめる先を検討してよい。まとめるかは作業の範囲に照らして決め、この判定だけを理由に無関係な変更へ広げない |
| `DO-NOT-EXTRACT` | **共通化しない**。似ているのは偶然で、意味が違う | **何も変更しない。「直せ」ではない。** 2 つをまとめる変更を始めていたなら、それを取り下げる |
| `REVIEW` | 判断材料が足りない | 共通化もその取り下げも自分で決めず、人に判断を仰ぐ。ただし下の注意 |

**構造が似ていないペアも `REVIEW` になる。** `compare` で名指ししたペアの
`structural-similarity` が `threshold` に届いていない（`lean` が `toward-extract` でない）なら、
そもそも共通化を考えるほど似ていない。人に投げずに「似ていない」として扱う。
`scan` / `check` は閾値に届いた候補ペアしか出さないので、この形は `compare` にしか現れない。

**`DO-NOT-EXTRACT` を警告として読まない。** 重複を報告するツールの出力は「直せ」と読まれがちだが、
このラベルは逆で、**共通化すること自体が誤り**だと言っている。見たエージェントが共通化すると、
間違った抽象化を作ることになる。

## `status` の読み方

| `status` | 意味 | 次にできること |
|---|---|---|
| `measured` | 測れた | 値（`similarity` / `overlap` / `steps` / `unifiable` など）を読む。どのキーが並ぶかはシグナルごとに違う（スキーマの `reason` の `allOf`） |
| `unmeasurable` | 尋ねたが測れなかった | `reason` を見る。環境やコードを直すと測れるようになることがある（下の表） |
| `not-asked` | そもそも尋ねていない | 何もしない。候補ペアでない（`not-a-candidate`。既定でも並ぶ）・LSP に尋ねない走り方だった（`stage-1-only`）・葉の並びが揃わず比べなかった（`unaligned-tokens`）。後ろ 2 つは `--explain` のときだけ並ぶ |

**測れなかったシグナルは、どちらへも傾けていない**（`lean` は `neither`）。測れなかった数で
判定を弱めてはいないので、`REVIEW` は「材料が揃わなかった」結果として読む。

`unmeasurable` のうち、エージェントが手を打てるもの:

| `reason`（と付属のキー） | 打てる手 |
|---|---|
| `untraced-type-name`（`untraced: omitted-type-annotation`） | 省いた型注釈を書くと、型シグネチャが測れるようになる |
| `lsp-unusable` / `document-unopenable` / `workspace-root-undecidable` | LSP サーバが入っているか・ファイルを開けるかを確かめる。コードを直す話ではない |
| `project-unrooted` / `outside-project`（`project_markers`） | 並んだ印（`tsconfig.json` など）の下にファイルを置く・範囲に含める |
| `ambiguous-domain`（`path` / `domains`） | `dryguard.toml` の `[domains]` で、そのファイルが 2 つの宣言に当たらないようにする |
| `unreadable-declaration`（`line`） | その行の import 宣言を読める形にする |

ここに無い `reason` の多くは dryguard かサーバの側で測れなかったもので、利用者のコードを直しても
変わらないことが多い。直し先が言い切れるものだけを上に並べている。

## 終了コードとの関係

| 終了コード | stdout の JSON | 読み方 |
|---|---|---|
| 0 | ある | 判定を出し終えた |
| 1 | ある | `--fail-on do-not-extract` を付けていて、`DO-NOT-EXTRACT` が 1 件以上出た（候補ペアを LSP に尋ね切れたときだけ。尋ね切れなければ 2） |
| 2 | **あるときと無いときがある** | `--fail-on` を付けて LSP に尋ね切れなかったときは JSON がある。引数・設定の誤り・走査を始められないなど、判定の前に止まったときは無い（理由は stderr） |

**終了コード 1 は「直せ」ではない。** `--fail-on do-not-extract` が落とすのは、
**共通化すべきでない 2 つを共通化しかけていないか**を CI やフックで止めるため。
1 を受け取ったら、`DO-NOT-EXTRACT` のペアについて上の表どおり**何も変更しない**
（共通化を始めていたなら取り下げる）。重複を解消しに行かない。

終了コード全体の表は `README.md` にある。

## 版と破壊的変更

`schema_version` は整数で、**読む側が壊れる変更のときだけ**上がる。

**版を上げる変更**:

- キーの削除・改名
- 値の型・意味の変更（`similarity` を丸める、`line` を 0 始まりにする、など）
- 閉じた語彙（`verdict` / `status` / `lean`）への値の追加・削除
- 開いた語彙の既存の値の改名・削除、`status` の付け替え（ある `reason` を `not-asked` から
  `unmeasurable` へ移す、など）
- 既定で出ていたキー・根拠を、`--explain` のときだけ出るように移す（既定で読む側にとっては削除）
- 最上位の形の変更

**版を上げない変更**:

- キーの追加（既定・`--explain` のどちらでも）
- 開いた語彙への値の追加（`signal` の名前・`reason` / `unopened` / `untraced` の綴り・
  `skipped_files[].reason`）
- `suggestion` の文言
- 配列の並び（`candidate_pairs` / `reasons` / ドメインごとの件数）。**並びは契約に入らない**

**読む側への約束**:

- 知らないキーは無視する
- 知らない `signal` の根拠は飛ばしてよい。`lean` は閉じた語彙なので、向きだけは読める
- 知らない `reason` は `status` だけで扱う（`unmeasurable` なら「測れなかった」）
- 知らない `schema_version` なら読まずに止まる

**`reason` は 2 つの文脈で使っている。** `reasons[].value.reason`（シグナルが測れなかった理由）と
`skipped_files[].reason`（ファイルを飛ばした理由）は別の語彙で、後者には `status` が無い。

スキーマ（`json-output.schema.json`）と実装の食い違いは、`src/report/json.rs` のテストが
次の範囲で止める。

- 版（`schema_version` の `const`）と、閉じた語彙（`verdict` / `status` / `lean`）の値の集合
- キーの集合を**両方向**で（どちらかだけを直すと落ちる）: `pair` / `compare_output` / `scan_output` /
  `reason` / `value` / `location` / `divergent_leaf`
- キーを**実装 → スキーマの 1 方向**で（実装だけが足したキーで落ちる）: `walked` / `skipped_file` /
  `domain_count`

**値の型・範囲（`type` / `minimum`）と、`measured` のときに必須のキー（`if` / `then`）は
テストが当てていない。** JSON Schema の検証器を依存に持っていないため。
