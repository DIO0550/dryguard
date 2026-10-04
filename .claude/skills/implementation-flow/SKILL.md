---
name: implementation-flow
description: Issue に紐づく実装を、ゴールの確定 → タスクの分割 → 計画 → 計画の検証 → 実装 → 実装の検証 → PR → マージ後の追記の順で進める。各段で Issue に何を残し、どの規約を読むかを持つ。Issue から起動されて実装に着手するとき、または実装の進め方を確かめたいときに使う。TDD サイクルと品質チェックの中身は持たない。
---

# implementation-flow — Issue を判断の履歴にしながら実装する

**このスキルが持つのは段の順序と、各段で Issue に何を残すかだけ。** 何を守るかは
`AGENTS.md` と `rules/`、Rust 共通の TDD サイクル・テスト・コーディングの中身は
d-market-rust の `rust-rules-plugin`（`tdd` / `testing` / `coding-standards`）が持つ。
**ここには写さない** — 同じ規則が 2 箇所にあると片方だけ古くなる
（`AGENTS.md`「規約の持ち場」の「再掲しない」、`harness-record` スキルと同じ形）。

**このスキルは層 4（お願いベース）で、起こすフックは無い。** 読まれなかった回の保証は無く、
無条件に効くのは CI（層 1）と git hooks（層 2）だけ（`AGENTS.md`「強制力の序列」）。

**UI の表示確認の段は置かない。** dryguard は CLI で、見て確かめる画面が無い。

## 段と、Issue に残すもの

| 段 | Issue に残すもの |
|---|---|
| 0. 着手 | セッションの URL |
| 1. ゴールの確定 | 疑問点があれば、それだけ（止まる） |
| 2. タスクの分割 | 分けた / 分けなかった理由 |
| 3. 計画 | 計画・却下した案・その理由・完了条件 |
| 4. 計画の検証 | 採った指摘・採らなかった指摘とその理由 |
| 5. 実装 | 計画から外れた判断と、その理由 |
| 6. 実装の検証 | 採った指摘・採らなかった指摘とその理由 |
| 7. PR | 手戻り・フックの発火など、記憶からしか取れないこと |
| 8. マージ後の追記 | 閉じる理由・残タスクの Issue へのリンク |

**PR 本文は差分の説明、Issue は判断の履歴**（`AGENTS.md`「実装の進め方」）。
**Issue に書く数値・主張は、書く直前に一次情報から出し直す**
（`AGENTS.md`「記録・PR 本文に数値や主張を書く前に」）。

## 手順

### 0. 着手

Issue に Claude Code セッションの URL をコメントする
（`AGENTS.md`「Issue に紐づいて起動したら、セッションの URL を Issue に残す」）。
**URL を組み立てられなければ推測で書かない。**

続けて `echo hook-canary` を 1 度走らせ、結果を Issue に書く（`.claude/hooks/README.md`
「`hook-canary.sh` — 発火しているかを確かめる」は push の前に走らせることを求めており、
最初の push より前に置けるのはここ）。通ったときの書き方は `AGENTS.md`「実装を始める前に」の
3 項目目に従う。

### 1. ゴールの確定

- Issue の本文とコメントを全部読む。親 Issue・Epic があればそれも読む
- `docs/dryguard-plan.md` の該当節を読む（`AGENTS.md`「実装を始める前に」の 2 項目目）。
  置き場所は多くの場合そこに書いてある
- **Issue が挙げた対象の一覧を鵜呑みにしない。** 完了条件を grep などで機械的に確かめられる
  形に落とし、自分で数え直す（同じ節の 1 項目目）
- **疑問点があれば実装に入らず、Issue に選択肢と根拠を書いて止まる。** 推測で埋めない。
  止まるコメントにもセッションの URL を併記する

### 2. タスクの分割

`AGENTS.md`「タスクの分割」が判断軸（独立してマージできるか）と書き残すことを持つ。
**分けないと決めた場合も、その理由を Issue に書く。**

### 3. 計画

- 置き場所: `rules/architecture.md`「3 ステージのパイプライン」の責務の表と
  「依存方向のルール（必須要件）」に照らす
- **影響範囲をファイル単位で割る。** 変える型・関数の参照元・既存テスト・既存コメントを
  grep で数える（計画の入力の網羅漏れは `plan` の分類として繰り返し記録されている）
- **実測で確かめずに置いた前提を残さない。** LSP サーバの応答・件数・実コーパスの数は、
  走らせて出す
- 先に書くテストを決める。`classification` の閾値・決定木を変えるなら、変えたい振る舞いを
  表すテストが先（`rules/tdd.md`「判定ルールを変えるときは、先にテストを足す」）
- **ステージをまたぐ移動・既存モジュールの再配置は、選択肢と根拠を Issue に書いて止まる**
  （`AGENTS.md`「設計判断の確認」）。止まるコメントにもセッションの URL を併記する
- 計画・却下した案・その理由・完了条件を Issue に書く

### 4. 計画の検証

サブエージェントに [`references/plan-review.md`](references/plan-review.md) の観点で
検証させる。**Issue の本文と計画をそのまま渡す**（サブエージェントは Issue を読めないことがある）。

返ってきた指摘は**一次情報で確かめてから**採否を決める。サブエージェントの数値・主張を
そのまま Issue に転記しない。採った指摘・採らなかった指摘とその理由を Issue に書き、
計画を直す。

### 5. 実装

- TDD のサイクル: プラグインの `tdd` スキル。dryguard 固有の足し分は `rules/tdd.md`
- 守るもの: `rules/*.md`（`CLAUDE.md` が常時読み込む）とプラグインの `coding-standards` / `testing`
- **`rules/*.md` が指す関数名・語の定義を変えたら、同じ PR で `rules/` の記述を出し直す**
  （`AGENTS.md`「実装を変えたら、rules/ 配下の記述も同じ PR で出し直す」）
- 依存を足す・上げるときは `harness/deps/resolve.sh` / `resolve-node.sh` を通す
  （`AGENTS.md`「依存の更新は cooldown を通す」）
- 計画から外れた判断は、その理由と一緒に Issue に書く

### 6. 実装の検証

**検査を 1 つずつ独立に走らせ、それぞれの結果行を見る**（理由は `harness/githooks/pre-push` の
冒頭のコメント）。コマンドは `AGENTS.md`「Common Commands」。

| 変えた場所 | 走らせるもの |
|---|---|
| 常に | `harness/githooks/pre-push` が走らせる fmt / clippy / test |
| LSP を通る経路 | `cargo test -- --ignored`。サーバが無くて走らせられなければ、**飛ばしたと書く**（`rules/testing.md`「LSP を要するテストは、飛ばしたことが分かる形にする」） |
| `.claude/hooks/` | 変えたフックの `*-test.sh`。`.claude/hooks/lib/` を変えたら全部の `*-test.sh` |
| `harness/ci/` | `harness/ci/pr-body-diff-test.sh` |
| テストの無いスクリプト（`harness/githooks/pre-push`・`harness/deps/*.sh`・`harness/records/count.sh`・`.claude/hooks/wire-githooks.sh`） | 手で走らせ、**何を走らせて何を見たか**を PR 本文に書く |

pre-push が走らせるのは 3 つだけで、残りは CI（`.github/workflows/`）にしか無い。
**CI にしか無いものを push の前に走らせないと、push の後に 1 巡を失う。**

**疑わしい assert は、その 1 件だけ実装を壊して走らせる**
（`rules/testing.md`「迷ったら、その 1 件だけ実装を壊して走らせる」）。

そのうえで、サブエージェントに [`references/implementation-review.md`](references/implementation-review.md)
の観点で差分を検証させる。扱いは計画の検証と同じ（一次情報で確かめて採否を決め、Issue に書く）。

### 7. PR

- 0 で走らせた `echo hook-canary` の結果を PR 本文にも書く
- **本文・コミットメッセージ・レビュー返信に書いた「N 件」「N 回」は、書く直前に走らせた
  コマンドの出力と 1 つずつ突き合わせる**（`unverified-claim` が `count.sh` の再発数 141 で
  最多。書いた内訳の合計が総数に届かない・前の巡の数の転記・別のツリーで数えた数が
  繰り返し出ている）。push のたびに、そのコミットで動いた数を本文の頭から読み直す
- 本文の行の頭に `差分規模: <ファイル数> ファイル / +<追加行> -<削除行>` を置き、
  push の前にローカルで突き合わせる

  ```bash
  bash harness/ci/pr-body-diff.sh <本文のファイル> origin/main HEAD
  ```

- **push するたびに、その push で古くなった記述を本文の頭から読み直す**
  （`AGENTS.md`「記録・PR 本文に数値や主張を書く前に」）
- 手戻り・フックの発火など、**記憶からしか取れないこと**を PR を出す前に Issue へ書く。
  判断の履歴ではないが、マージ後の記録は別のセッションで書かれることがあり、
  そのセッションが読める一次情報の置き場として Issue を使う
  （`harness/records/README.md` の「書き方」の「推測で埋めない」）

### 8. マージ後の追記

- 記録は `harness-record` スキルに渡す。書き方はここに写さない
- Issue を閉じる。**残タスクが 1 つでもあれば、閉じる前に新しい Issue を作ってリンクし、
  スコープ外にしたことを書く**（`AGENTS.md`「Issue は完了したら閉じる」）

**移植元は読んでいない。** design-composer の `implementation-flow` と d-market-rust の
`implementation-workflow` は、書いたセッションからは読めなかったため新規に書いた。
