#!/usr/bin/env bash
#
# 依存を解決する cargo コマンドの唯一の入口。cooldown（min-publish-age）を効かせて走らせる。
#
# Why: 悪意あるバージョンは build script と proc macro が `cargo build` の時点でローカル実行される。
# 掴んでから CI で気付いても手遅れなので、解決そのものを止める側に置く。
#
# Why not（CI で Cargo.lock の pubtime を検査する）: 赤くなる頃には自分のマシンで実行済み。
# 事後のバイパス検知にはなるが、防御にはならない。
#
# 使い方:
#   bash harness/deps/resolve.sh update
#   bash harness/deps/resolve.sh update -p clap
#   bash harness/deps/resolve.sh add serde
#   bash harness/deps/resolve.sh update --manifest-path harness/phase0/chunk-unit-probe/Cargo.toml
#
# cargo のオプションはサブコマンドの後ろに書く。渡した引数はそのまま cargo に流すので、
# 前に置くと -Zmin-publish-age と並んでサブコマンドより前に出てしまう。
set -euo pipefail

# 代入で受けてから cd する。`cd "$(git ...)"` は git が失敗しても `cd ""` が成功するため、
# リポジトリの外で黙って別のディレクトリのまま走る。
repository_root="$(git rev-parse --show-toplevel)"
cd "$repository_root"

if [ "$#" -eq 0 ]; then
  echo "resolve: cargo のサブコマンドを指定してください（例: resolve.sh update）" >&2
  exit 1
fi

# 期間の値が無ければ既定は "0"（無効）。設定ファイルごと消えても走ってしまう形にしない。
if ! grep -q "global-min-publish-age" .cargo/config.toml 2>/dev/null; then
  echo "resolve: .cargo/config.toml に global-min-publish-age がありません" >&2
  echo "resolve: 期間の指定が無いと cooldown は既定の 0（無効）になるため、解決しません" >&2
  exit 1
fi

# nightly が無ければ落とす。stable では min-publish-age が無言で無効になるので、
# 黙って stable にフォールバックすると「走ったが守られていない」が作れてしまう
# （AGENTS.md「強制力の序列」のフェイルオープンかつサイレント）。
if ! cargo +nightly --version >/dev/null 2>&1; then
  echo "resolve: nightly ツールチェーンがありません" >&2
  echo "resolve: min-publish-age は nightly でしか効かないため、stable では解決しません" >&2
  echo "resolve: rustup toolchain install nightly を実行してください" >&2
  exit 1
fi

echo "resolve: cargo +nightly -Zmin-publish-age $*"
exec cargo +nightly -Zmin-publish-age "$@"
