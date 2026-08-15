# harness/githooks — 実行環境に依存しない push 前検査

git ネイティブのフック置き場。`core.hooksPath` をここへ向けると、CLI・IDE・素の git の
どれから push しても同じ検査が走る。

## 何が走るか

| フック | 検査 |
| --- | --- |
| `pre-push` | `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test` |

`cargo` が無い環境では、何も検査せずに通す。検査できないことを理由に push を止めても
検査の質は上がらないため。無条件のゲートは CI（層 1）にある。

## 配線 — cargo に `prepare` が無い

移植元（design-composer）は `package.json` の `prepare` が `pnpm install` のたびに
配線していた。**cargo にこれに当たるフックは無い**ので、同じ手が使えない。

**手で 1 度実行する形は採らない。** それは移植元で実際に穴になった形で、
リモート実行環境は毎回クローンからやり直すうえ DevContainer の `postCreateCommand` も
走らない。そこは Claude Code のフックが読まれない環境と同じなので、**層 2 と層 3 が
同時に抜ける**。

代わりに、**配線する入口を重ねる**。どちらも [`set-hooks-path.sh`](set-hooks-path.sh) を呼ぶ。

| 入口 | 効く環境 | 状態 |
| --- | --- | --- |
| Claude Code の `SessionStart` フック | Claude Code のセッションを開いたとき | 配線済み（`.claude/hooks/wire-githooks.sh`） |
| DevContainer の `postCreateCommand` | DevContainer でコンテナを作ったとき | **未配線** |

DevContainer 側は `.devcontainer/devcontainer.json` の `postCreateCommand` の末尾へ
次を足すと配線される。

```
; bash harness/githooks/set-hooks-path.sh
```

`&&` ではなく `;` で繋ぐ。前段のセットアップが失敗しても配線だけは通したいため。

```bash
# 配線されているかの確認
git config --get core.hooksPath   # → harness/githooks
```

## 配線の穴と、それを誰が塞ぐか

**単独で穴の無い入口は無い。** 残る穴を書き出しておく。

| 状況 | 層 2 | 層 3 | 塞ぐもの |
| --- | --- | --- | --- |
| DevContainer で開いた | 効く | 効く | — |
| Claude Code のセッション（フックが読まれる） | 効く | 効く | — |
| Claude Code のセッション（フックが読まれない） | **抜ける** | **抜ける** | **CI（層 1）** |
| 素の `git clone` + 別のクライアントから push | **抜ける** | **抜ける** | **CI（層 1）** |

**穴が残ることを承知で層 2 を置く。** 層 2 は「push の前に気づける」ための層で、
**ゲートそのものではない**。ゲートは層 1 にあり、そこは無条件に効く。

**Why not（CI で配線の有無を検査する）**: `core.hooksPath` はクローンごとのローカル設定で、
リポジトリには痕跡が残らない。CI が見られるのは push されたものだけなので、検査できない。
同じ理由で、配線されていないことに push 前に気づく方法も無い。

## なぜ git 側にも置くのか

**Claude Code のフックは発火しない実行環境がある。** リモート実行環境では
`.claude/settings.json` の配線が読み込まれないことがあり、しかも**フェイルオープンかつ
サイレント**なので、通ったのか検査されなかったのかが区別できない。

強制力の序列は `AGENTS.md`「強制力の序列」。

## 動作確認

```bash
bash harness/githooks/pre-push
```

`core.hooksPath` を設定したうえで push すると、失敗した検査の出力がそのまま出て
push が中止される。
