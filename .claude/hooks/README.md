# .claude/hooks — Claude Code のフック（層 3）

`.claude/settings.json` から配線する。**どれも enforcement として数えない** —
フック自体が読まれない実行環境があり、しかもフェイルオープンかつサイレントなので、
通ったのか検査されなかったのかを区別できない。ゲートは層 2（`harness/githooks/`）と
層 1（`.github/workflows/`）にある。

## 何が走るか

| フック | イベント | すること |
| --- | --- | --- |
| `wire-githooks.sh` | `SessionStart` | `core.hooksPath` を `harness/githooks` へ向ける（`harness/githooks/README.md`「配線 — cargo に `prepare` が無い」） |
| `pre-push-check.sh` | `PreToolUse`（`Bash`） | コマンドに `git push` があれば `harness/githooks/pre-push` を走らせ、落ちたら exit 2 で止める |

## `pre-push-check.sh` — 層 2 との違いは発火条件だけ

**検査の本体を二重管理にしない。** 呼ぶのは `harness/githooks/pre-push` そのもので、
fmt / clippy / test の並びを持つのはそこだけ。こちらが足すのは、**Bash のコマンドが
実行される前に**、落ちた検査の出力を Claude に返すことだけ。

**`core.hooksPath` が配線されていても走る。** 同じ push で検査が 2 回走るが、
配線済みなら飛ばす形にすると `git push --no-verify` が素通りする。

**`git push` の見つけ方**（`lib/git-push-command.sh`）:

- 引用符の中身を落としてから `&&` / `||` / `;` / `|` / `&` / 括弧 / 改行で区切り、
  区切った 1 つずつの**先頭**が `git push` かを見る。環境変数の代入と `git` の大域オプション
  （`-C <dir>` / `-c <k=v>` / その他の `-*`）は飛ばす
- **読み損ねた形は「push ではない」側へ倒す。** 取りこぼしても層 2 が push の時点で
  同じ検査を走らせるが、push でないコマンド（`git commit -m "then git push"` など）を
  push と読むと、検査の失敗でそのコマンドまで止めてしまう

**黙って通す条件**: `jq` が無い・入力の JSON が読めない・`CLAUDE_PROJECT_DIR` が
git の作業ツリーでない。`cargo` が無い環境は `harness/githooks/pre-push` 自身が通す。

**タイムアウトは 600 秒**（`.claude/settings.json`）。`cargo test` がビルドから始まると
長くかかるので、既定に頼らず明示する。超えたときはフェイルオープンで通る。

## 強制力の序列

`AGENTS.md`「強制力の序列」が持つ。

## テスト

```bash
bash .claude/hooks/pre-push-check-test.sh
```

CI（`.github/workflows/rust.yml` の `claude-hooks`）でも走る。
