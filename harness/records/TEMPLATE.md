# 記録のテンプレート

`harness/records/pr-<番号>.md` へコピーして使う。

## Contents

- テンプレート
- 層の語彙
- 分類の語彙

---

## テンプレート

````markdown
# PR #<番号> <タイトル>

- マージ日: <YYYY-MM-DD>
- 関連 Issue: #<番号>
- 差分規模: <変更ファイル数> ファイル / +<追加行> -<削除行>

## ゴールと結果

<Issue に書いたゴールと、実際に達成できたか。ずれていたらそのずれ>

## 指摘

### 1. <一行で内容>

- 分類: `<語彙>`
- 出どころ: レビュー / CI / 自己修正 / フック
- 内容: <何が起きたか>
- 既存ルール: rules/<file>.md「<見出し>」 / プラグイン `<スキル名>` / なし
- 次にどう防ぐか: <ルール追記 / 観点に追加 / フック追加 / 今回は記録のみ>

### 2. <一行で内容>

...

## 手戻り

<計画から外れた箇所と、外れた理由。無ければ「無し」>

## うまくいったこと

<次も同じようにやりたいこと。無ければ「無し」>

## 規約への反映

<介入した分類ごとに、置いた場所と、それより上の層を採らなかった理由。
 介入が無ければ「無し」とその理由>

**この回の介入（`count.sh` が数える行）:**

- 対策済: `<分類>` 層=<hook|skill|観点|rules> at pr-<番号>
````

**`対策済` の行を書くのは介入した回だけ。** `count.sh` はこれを「ここから数え直す」の
起点として読むので、介入していない回に書くと再発数が 0 に戻る。
記録を書く時点では、介入が無ければ「規約への反映」に**無しとその理由**を書いて終える。

---

## 層の語彙

`対策済` の行に書く `層=` はこの 4 つ。**強制力の強い順**
（`AGENTS.md`「強制力の序列」）。

| 層 | 置いた場所 |
| --- | --- |
| `hook` | CI（`.github/workflows/`）・git hooks（`harness/githooks/`）・`.claude/hooks/`・`Cargo.toml` の `[lints.clippy]` |
| `skill` | `.claude/skills/` の新しいスキル |
| `観点` | 既存スキルの手順・観点 |
| `rules` | `rules/` への追記・表現の修正 |

---

## 分類の語彙

`分類` はここから選ぶ。**勝手に増やさない。** どれにも当てはまらないなら `なし` と書く
（それが規約の抜けの候補になる）。

**この表は `rules/` の見出しから機械的に引いたもの**で、まだ 1 件も記録がついていない。
実際に指摘が溜まると、1 つのタグに別種の問題が畳まれていることが分かる。
そのときに分割する（過去の記録は書き換えない）。

| 分類 | 対応する規約 |
| --- | --- |
| `layer-dependency` | rules/architecture.md「依存方向のルール」 |
| `verdict-placement` | rules/architecture.md「判定は 1 箇所にだけ置く」 |
| `missing-signal` | rules/architecture.md「取れなかったシグナルを既定値で埋めない」 |
| `module-api` | rules/architecture.md「モジュールの公開 API」 |
| `result-option` | rules/coding.md「不在は `Option`、失敗は `Result`」 |
| `error-variant` | rules/coding.md「エラー型は原因ごとにバリアントを分ける」 |
| `smart-constructor` | rules/coding.md「生成時に検証し、不正な値を存在させない」 |
| `type-vocabulary` | rules/coding.md「値の語彙を型で閉じる」 |
| `illegal-state` | rules/coding.md「不正な状態を型で表現できなくする」 |
| `lint-suppress` | rules/coding.md「禁止事項」（lint 抑制を足さない） |
| `comment-mismatch` | rules/coding.md「コメントは doc と Why / Why not に絞る」（内容が実装・事実と食い違う） |
| `comment-missing` | rules/coding.md「Why と Why not は別物として書く」（書くべき Why / Why not が無い） |
| `naming-mismatch` | rules/naming.md「名前と実体を一致させる」 |
| `naming-vocabulary` | rules/naming.md「このツールの語彙を固定する」（同じものを別名で呼んだ） |
| `test-assert-weak` | rules/testing.md「assert は『落ちうるか』で見る」 |
| `test-coverage-gap` | rules/testing.md「振る舞いをテストする」（差分の中心がどのテストからも参照されていない） |
| `test-mock` | rules/testing.md「モックは使わない」 |
| `duplication-test` | rules/testing.md「テスト用ヘルパーの置き場所」 |
| `duplication-logic` | 同じ処理が 2 箇所に現れた（テスト以外） |
| `tdd-order` | rules/tdd.md「判定ルールを変えるときは、先にテストを足す」 |
| `ownership` | プラグイン `coding-standards`（所有権・借用の設計） |
| `over-guard` | 過剰なブロック / フォールバック |
| `plan` | 計画の誤り・不足 |
| `unverified-claim` | AGENTS.md「記録・PR 本文に数値や主張を書く前に」（PR 本文・Issue コメント・コミットメッセージ・レビュー返信・通知に書いた数値や主張が、一次情報から出し直されていない） |
| `なし` | 既存の規約に対応が無い（= 規約の抜けの候補） |

`分類: なし` が 10 件溜まったら語彙へ昇格させる。
