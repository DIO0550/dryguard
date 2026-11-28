# AGENTS.md — 実装規約

このリポジトリで実装を行う AI エージェントは、以下の規約に**必ず**従うこと。
各規約は `rules/` 配下にあり、以下の `@` import で常にコンテキストへ読み込まれる。

## プロジェクト構成

- 何を作るか: **構造の似たコードが偶発的な重複かどうかを、型・依存・参照の意味情報から
  理由付きで判定する CLI**（`docs/dryguard-plan.md`）
- 言語: Rust（edition 2024）
- 構成: `src/lib.rs` が本体、`src/main.rs` は引数を受けて呼ぶだけ
- 外部依存: `clap` / `tree-sitter` / `tree-sitter-typescript` / `tree-sitter-rust` / `tree-sitter-java` / `tree-sitter-swift` / `rayon` / `lsp-types` / `serde_json`
- LSP サーバ: TS は typescript-language-server、Rust は rust-analyzer、Swift は sourcekit-lsp、Java は jdtls（子プロセスで起動）

## 規約一覧

- @rules/architecture.md — 3 ステージの依存方向・判定の置き場所・モジュールの公開 API
- @rules/coding.md — エラーと不在の表現・生成時の検証・値の語彙を型で閉じる・コメント
- @rules/naming.md — 名前と実体の一致・このツールの語彙の固定
- @rules/testing.md — 振る舞いのテスト・assert が落ちうるか・モック禁止
- @rules/tdd.md — どこに TDD が効くか・判定ルールを変えるときの順番

## 規約の持ち場

**言語共通の Rust 規約は d-market-rust の `rust-rules-plugin` が持ち、
dryguard 固有の規約は `rules/` が持つ。**

| 置き場所 | 持つもの |
|---|---|
| プラグイン（`coding-standards` / `tdd` / `testing`） | 所有権・借用・`Result` / `?`・テスト構造・TDD サイクルなど、Rust プロジェクトなら共通のもの |
| `rules/`（ここ） | 3 ステージの構造・このツールの語彙・判定の置き場所、およびプラグインに**足す / 逸脱する**分 |

- **同じ話題が両方に出たら `rules/` が勝つ。** ただしそれは逸脱なので、
  `rules/` 側に**なぜ逸脱するのかを書く**
- **プラグイン側にあることを `rules/` に再掲しない。** 二重管理になり、片方だけ古くなる。
  各 `rules/*.md` の冒頭に「前提として守るもの」を箇条書きで置き、詳細はプラグインへ委ねる

**Why not（プラグインのみ / `rules/` のみ）**: プラグインだけだと dryguard 固有の構造規約が
置けず、`@` import で常時読み込む形にもできない。`rules/` だけだと d-market-rust の更新が
届かず、他の Rust プロジェクトと規約を育て合えなくなる。

## 規範と判例

常時読む `AGENTS.md` と `rules/` に判断基準を残す。計画・報告・規約文書の同期で起きた実例は
[判例](harness/case-law/README.md) に置き、検証する観点に当たるときや、その分類で迷ったときに読む。

## 実装の進め方

ゴールの確定 → タスクの分割 → 計画 → 計画の検証 → 実装 → 実装の検証 → PR →
マージ後の追記、までが 1 セット。段ごとの手順と、各段で Issue に残すものは
`implementation-flow` スキル（`.claude/skills/implementation-flow/SKILL.md`）が持つ。

**計画・却下した案・その理由は、すべて Issue に追記する。** PR 本文は差分の説明、
Issue は判断の履歴、という分担にする。採用した案だけを残すと、後で同じ案が再浮上した
ときに前回やめた理由が失われる。

マージ後はその回の評価を `harness/records/` に記録する。**書き方と、記録を作らない例外
（`harness/records/` 配下だけを変える PR）は `harness/records/README.md`** が持つ。
記録を数えて規約やフックへ手を入れるのは、別の機会に行う。

## タスクの分割

**タスクが大きくなりそうな場合は、Issue を分離して新たに登録する。**

- 判断軸は「**独立してマージできるか**」。片方だけ入っても壊れない単位が 2 つ以上見えたら分ける
- 分けたら、元の Issue に**分割した理由**とリンクを残す。分けた側にも「何をスコープ外にしたか」を書く
- **分けないと決めた場合も、その理由を Issue に書く。** 分けないこと自体が判断なので、記録の対象になる

## Issue は完了したら閉じる

**その Issue で決めたことをやり終えたら閉じる。残ったタスクは新しい Issue にする。**

- **「直さない」も完了。** 測った結果・却下した案・その理由をコメントに残したうえで閉じる
- **残タスクが 1 つでもあれば、閉じる前に新しい Issue を作ってリンクする。**
  「あとでやる」を元の Issue のコメントに残さない
- 新しい側には**何をスコープ外にしたか**を書く（「タスクの分割」と同じ形）

**Why**: 開いたままの Issue は「まだ何か残っている」と読める。判断が済んでいるのに開いていると、
次に見た人が**どこまで決まったのかをコメントの並びから読み直す**ことになる。

**Why not（残タスクがあるあいだ開けておく）**: 1 つの Issue が複数の未完了を抱えると、
**どれが終わってどれが残っているかが構造に出ない**。コメントを最後まで読んで突き合わせることになり、
Issue の一覧（開いているか閉じているか）からは読めない。

## 実装を始める前に

ここに入るのは、**このリポジトリのレビューで実際に 2 回以上出た指摘**だけ。

- **計画の入力を鵜呑みにしない。** Issue が挙げた対象の一覧は「調べた結果」であって
  網羅の保証ではない。完了条件を grep 等で機械的に確かめられる形に落とし、自分で数え直す
- **計画を立てる前に `docs/dryguard-plan.md` の該当節を読む。** 実装の置き場所は
  多くの場合そこに書いてある
- **`echo hook-canary` が deny されずに通っただけでは「フックが発火しない環境」と
  断定しない。** **セッションを起動した時点の** `.claude/settings.json` の `PreToolUse` に
  `hook-canary.sh` が配線されていたかを確かめ、配線されていなければ「canary が無いので層 3 を
  判定できなかった」と書く

**Why not（他リポジトリの項目をコピーしない）**: 移植元（design-composer）の
自己チェック項目は、そのリポジトリで繰り返された失敗から育ったもの。
自分たちの失敗から来ていない項目を並べると、読む側が「これは守る意味があるのか」を
判断できず、**規約全体の信頼が落ちる**。上の項目ができた経緯は [計画の判例](harness/case-law/process.md#plan) を参照。

## 記録・PR 本文に数値や主張を書く前に

ここに入るのも、**2 回以上出た指摘**だけ。

- **記録・PR 本文・Issue コメント・コミットメッセージ・レビュー返信・通知に書く数値や主張は、
  書く直前に一次情報（`git diff` / `git log` / テスト出力 / `count.sh` の出力）から
  出し直す。** 記憶・前の記録・前の巡のレビュー返信からの転記のまま書かない
- **関連する変更が入った後と最終提出の前にも出し直す。**
- **出し直す時点を「push の後」に置かない。push するたびに、その push で古くなった記述を
  本文の頭から読み直す。**
- **差分規模だけは層 1 が無条件に突き合わせる。** 本文の**行の頭**に
  `差分規模: <ファイル数> ファイル / +<追加行> -<削除行>` を置く
  （箇条書きの `- ` と太字の `**` は付いていてよい。2 行以上書くなら数を揃える。
  `.github/workflows/pr-body.yml` / `harness/ci/pr-body-diff.sh`）。
  引用した文言・数えた件数・主張の条件は、書く側が一次情報と突き合わせる。

記録を書くときは、[記録の書き方](harness/records/README.md#書き方) の「推測で埋めない」も適用する。
分類は [記録テンプレート](harness/records/TEMPLATE.md) に従う。hook-canary の誤断定は `plan` を使う。
実例は [裏取り漏れ](harness/case-law/process.md#unverified-claim) と
[後続の変更で古くなった報告](harness/case-law/process.md#claim-staleness) を参照。

## 実装を変えたら、rules/ 配下の記述も同じ PR で出し直す

ここに入るのも、**2 回以上出た指摘**だけ。

`rules/*.md` が指す語彙の定義・関数名・振る舞いを変えたら、同じ PR で規約文書を読み直して
出し直す。レビュー対応で実装を直したときも行う。実例は [規約文書の同期](harness/case-law/process.md#rule-staleness) を参照。

## 規約の更新

レビューで新しい判断基準が示されたら、その場の修正で終わらせず **`rules/` 配下へ反映**する。
ルールに書かれていない指摘が **2 回以上**出たら、規約の抜けとして扱う。

**この「`rules/` 配下へ反映」は dryguard 固有の構造・語彙の規約を指す。** 進め方・報告の
書き方など、`rules/` に置き場所が無い判断基準は、この `AGENTS.md` の専用の節
（「実装を始める前に」「記録・PR 本文に数値や主張を書く前に」「実装を変えたら、
rules/ 配下の記述も同じ PR で出し直す」）に置く。

回数を数える材料は `harness/records/` に溜まる。数えるのは `harness/records/count.sh`。
**通算ではなく「最後の介入以降の再発数」で見る**（通算は単調増加するので、介入が効いたかを表さない）。
同じ層で再発したら層を 1 つ上げる。

**規約が増えたら、それを理由に見送ったフックを見直す。** 見送りの理由は
「今の規約に無いから」であることが多く、規約が変わると理由ごと消える。

## 設計判断の確認

ステージをまたぐ移動や既存モジュールの再配置など、**他の規約と衝突しうる変更**は、
実装前に選択肢と根拠を示して確認する（勝手に進めず、判断だけを仰ぐ）。

## Issue に紐づいて起動したら、セッションの URL を Issue に残す

Issue に紐づく作業でセッションが始まったら、**着手した時点で**その Issue へ
Claude Code セッションの URL をコメントする（`https://claude.ai/code/session_<id>`）。
判断待ちで止まるときは、**選択肢と根拠を書いたコメントに改めて併記する**。

- **理由: 経緯はそのセッションの中にしか無い。** どこまで読んだか・何を確かめて選択肢を
  絞ったかは Issue のコメントに書ききれず、Issue だけを見ている人と、あとから引き継ぐ
  **別のセッション**が辿れなくなる
- **止まってから残すのでは遅い。** 止まるかどうかは着手時には決まっていないので、
  起点を「止まったとき」ではなく「着手したとき」に置く
- 通知にだけ載せるのでは足りない。**通知は流れるが Issue は残る**

**URL は `SessionStart` のフック（`.claude/hooks/session-url-notice.sh`）がコンテキストに出す。**
URL はセッションの中からしか作れないので、規約だけだと「残そうと思ったが URL が分からない」で
止まる。ブランチが `claude/issue-<N>-...` なら Issue 番号も添える。**URL を組み立てられないと
出たら、推測で書かない**（フックの出力が無いときも同じ。層 3 は発火しない環境がある）。

## Common Commands

リポジトリルートで実行する。

```bash
cargo build                                 # ビルド
cargo run -- compare <locA> <locB>          # 実行
cargo fmt                                   # フォーマット
cargo fmt --check                           # フォーマット検査（CI と同じ）
cargo clippy --all-targets -- -D warnings   # Lint（テストコードも対象・警告をエラー扱い）
RUSTDOCFLAGS="-D rustdoc::private_intra_doc_links" cargo doc --no-deps  # doc 検査（CI と同じ）
cargo test                                  # テスト
bash harness/githooks/pre-push              # push 前の検査をまとめて走らせる
bash harness/ci/pr-body-diff-test.sh        # PR 本文の差分規模を突き合わせる検査のテスト
bash .claude/hooks/pre-push-check-test.sh   # 層 3 の push 前検査フックのテスト
bash .claude/hooks/post-edit-rust-test.sh   # 層 3 の編集後 fmt / clippy フックのテスト
bash harness/githooks/probe-check-test.sh  # 層 2 の pre-commit が一時モジュール・生成した Cargo.lock を止めるかのテスト
bash harness/githooks/stale-doc-check-test.sh  # 層 2 の定義を触った識別子を名指しする差分外のコメントを出すチェックのテスト
bash .claude/hooks/hook-canary-test.sh      # 層 3 のカナリア（フックの発火の確認）のテスト
bash .claude/hooks/session-url-notice-test.sh  # 層 3 のセッション URL の通知のテスト
bash .claude/hooks/post-merge-review-test.sh   # 層 3 のマージ後の振り返りを促すフックのテスト
bash .claude/hooks/pr-body-check-test.sh       # 層 3 の PR 本文の差分規模の宣言を作成前に確かめるフックのテスト
bash .claude/hooks/issue-comment-count-check-test.sh  # 層 3 の Issue コメントの件数に出どころを求めるフックのテスト
bash .claude/hooks/plan-impact-check-test.sh   # 層 3 の計画のコメントに影響範囲の grep を求めるフックのテスト
bash .claude/hooks/plan-completion-check-test.sh  # 層 3 の計画のコメントに完了条件のコマンドと期待する出力を求めるフックのテスト
bash .claude/skills/discord-notify/send-test.sh  # Discord の通知を上限内に割るスクリプトのテスト
```

PR 本文の差分規模の突き合わせ自体は、本文をファイルに落として走らせる
（CI が PR ごとに走らせるのと同じもの）。

```bash
bash harness/ci/pr-body-diff.sh <本文のファイル> origin/main HEAD
```

## 強制力の序列

**Claude Code のフックは発火しない実行環境がある。** リモート実行環境では
`.claude/settings.json` の配線が読み込まれないことがあり、しかも**フェイルオープンかつ
サイレント**なので、通ったのか検査されなかったのかを区別できない。
したがって**フックを enforcement の最上位として数えない**。

| # | 層 | 効く範囲 | タイミング | 置き場所 |
| --- | --- | --- | --- | --- |
| 1 | CI | 無条件 | push の後 | `.github/workflows/` |
| 2 | git hooks | クライアント非依存 | push の前 | `harness/githooks/` |
| 3 | Claude Code hooks | CLI 起動セッションのみ | 編集・コマンドの直前 | `.claude/hooks/` |
| 4 | skill / rules | お願いベース | 読まれたとき | `.claude/skills/` / `rules/` |

**ツールで落とせるものは規約の文に留めない。** `unwrap` / `expect` の禁止を
`Cargo.toml` の `[lints.clippy]` に置いてあるのがこの適用
（層 1 の `cargo clippy` が無条件に落とす）。

### CI で使うアクション

- **バージョンは commit hash で固定する。** `@v5` のようなタグは動かせるので、
  同じ設定でも別のコードが走りうる。**層 1 は無条件のゲートなので、何が走るかを
  固定できないと検査そのものが信用できない**
- 可読性のため hash の隣に `# v5.1.0` の形で版をコメントする。
  更新するときは hash とコメントの両方を直す
- **既定は GitHub 公式（`actions/*`）。** それ以外を使うなら、**commit hash で固定したうえで、
  そのアクションが何を固定できて何を固定できないかをワークフローに書く**
  （固定できない部分は、残った緩みとして読める形にしておく）
- 公式以外で `run:` に書き下せることは書き下す。ツールチェーンの用意は `rustup` を呼べば足りる

**今使っている公式以外のアクションは `pnpm/action-setup` の 1 つ。** 固定できる範囲は
2 段のうち 1 段目まで（bootstrap は同梱 lock で integrity 込み、`self-update` は版指定のみ）。
詳細は `.github/workflows/rust.yml` のコメント。

## 依存の更新は cooldown を通す

**`cargo update` / `cargo add` を素で叩かない。`harness/deps/resolve.sh` を通す**
（詳細は `harness/deps/README.md`）。

```bash
bash harness/deps/resolve.sh update
bash harness/deps/resolve.sh add serde
```

**Why**: 悪意あるバージョンは build script と proc macro が `cargo build` の時点で
ローカル実行される。**掴んだ時点で実行済み**なので、後段の層では止められない。
`.cargo/config.toml` の `global-min-publish-age` が、公開から日が浅いバージョンを
解決に使わないようにする。

**この件だけは強制力の序列が逆転する。** 序列は「検査が確実に走るか」で並んでいるが、
ここで要るのは「**コードが実行される前か**」なので、上位の層ほど手遅れになる。

| 層 | この用途 |
|---|---|
| 解決時（`min-publish-age`） | **ここだけが本当のブロック** |
| git hooks（層 2） | ビルド済み = 手遅れ |
| CI（層 1） | push 後 = 手遅れ。バイパスの事後検知にしかならない |

**stable では無言で無効になる**（`[unstable]` テーブルも `global-min-publish-age` も
警告を出さない）ため、`resolve.sh` は nightly が無ければ落ちる。
「設定してあるのに守られていない」を作らないのがスクリプトを挟む理由で、
**素の `cargo update` はそれを迂回する**。

### JavaScript 側のパッケージは pnpm で入れる

**`npm` は使わない。`pnpm` を使い、cooldown を 5 日にする**（`pnpm-workspace.yaml` の
`minimumReleaseAge: 7200`。分で指定する）。今の対象は CI が起動する
typescript-language-server と typescript で、**このリポジトリに JavaScript のコードは無い**。

**`pnpm install` / `pnpm add` を素で叩かない。`harness/deps/resolve-node.sh` を通す**
（詳細は `harness/deps/README.md`）。

```bash
bash harness/deps/resolve-node.sh install --lockfile-only
bash harness/deps/resolve-node.sh add -D <パッケージ>
```

**Why**: cooldown を掛けられるのが pnpm だけだから。npm には公開からの経過日数で
解決を止める設定が無く、Cargo 側（`global-min-publish-age`）と同じ守り方ができない。
**postinstall スクリプトは install の時点でローカル実行される**ので、cargo の build script と
同じく「掴んだ時点で実行済み」になる。

**`pnpm-lock.yaml` を commit する。** cooldown は「新しすぎるものを避ける」だけで、
**毎回同じものを入れる保証にはならない**。固定は lock が担い、cooldown は lock を
作り直すときに効かせる（`Cargo.lock` + `resolve.sh` と同じ分担）。
CI は `pnpm install --frozen-lockfile` で入れるだけなので、解決が起きない。

**pnpm の版を確かめてから解決する。** `minimumReleaseAge` を知らない版（10.16.0 より前）は
**その設定を警告なく無視する**。設定してあるのに守られていない状態を作らないため、
`resolve-node.sh` が版を見て落とす（`cargo` 側で nightly を要求しているのと同じ形）。

**効いている cooldown の長さも見る。** `resolve-node.sh` は `pnpm config get` が返す値が
5 日（7200 分）に届かなければ解決しない。**設定ファイルを読んで「書いてあるか」を見ない**
（コメントにだけ名前が残っていても、`0` や `1` でも通ってしまう）。期間を縮めるには
スクリプト側の下限も直すことになるので、**短くする変更は差分に出る**。

**CI が使う pnpm の版は `package.json` の `packageManager` が持つ。** `pnpm/action-setup`
（commit hash で固定）がそこを読んで入れ、ワークフローが**入った版を突き合わせる**。
版を上げるときに直すのはこの 1 箇所。

**中身までは固定できていない。** アクションは `pnpm self-update <版>` で目的の版を取るので、
**固定されるのは版だけ**（`packageManager` に `+sha512...` を付けてもアクション側が捨てる）。
バイナリをハッシュで固定していた形からは、ここだけ緩めた。

**Why not（curl + `sha256sum -c`）**: 走るバイナリの中身まで固定できるが、pnpm を上げるたびに
ハッシュを取り直すことになる。その手間と引き換えに 2 段目の固定を手放した。

**Why not（corepack）**: Node 25.0.0 から同梱されなくなった。それ以降に corepack を入れる
公式の方法が `npm install -g corepack` で、npm を使わない方針と衝突する。

**npm 禁止の対象は「このリポジトリの依存を解決すること」。** `pnpm/action-setup` は
bootstrap の pnpm を `npm ci` で入れるが、**同梱の lock に integrity があり解決は起きない**ので、
cooldown を掛けられないという禁止の理由がそもそも当たらない。
