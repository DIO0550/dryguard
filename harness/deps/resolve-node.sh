#!/usr/bin/env bash
#
# 依存を解決する pnpm コマンドの唯一の入口。cooldown（minimumReleaseAge）を効かせて走らせる。
# cargo 側の resolve.sh と同じ役割で、対象が JavaScript のパッケージになる。
#
# Why: postinstall スクリプトは install の時点でローカル実行される。掴んでから気付いても
# 手遅れなので、解決そのものを止める側に置く（cargo の build script と同じ形）。
#
# Why not（CI で pnpm-lock.yaml の公開日を検査する）: 赤くなる頃には自分のマシンで実行済み。
# 事後のバイパス検知にはなるが、防御にはならない。
#
# 使い方:
#   bash harness/deps/resolve-node.sh install --lockfile-only
#   bash harness/deps/resolve-node.sh update
#   bash harness/deps/resolve-node.sh add -D <パッケージ>
#
# lock から入れるだけ（解決を伴わない `pnpm install --frozen-lockfile`）はこれを通さなくてよい。
# 掴むものが lock で決まっており、cooldown の出番が無いため。
set -euo pipefail

# minimumReleaseAge を読むようになった最初の版。これより前は設定を警告なく無視する。
readonly REQUIRED_PNPM_VERSION=10.16.0

# 代入で受けてから cd する。`cd "$(git ...)"` は git が失敗しても `cd ""` が成功するため、
# リポジトリの外で黙って別のディレクトリのまま走る。
repository_root="$(git rev-parse --show-toplevel)"
cd "$repository_root"

if [ "$#" -eq 0 ]; then
  echo "resolve-node: pnpm のサブコマンドを指定してください（例: resolve-node.sh update）" >&2
  exit 1
fi

if ! command -v pnpm >/dev/null 2>&1; then
  echo "resolve-node: pnpm がありません" >&2
  echo "resolve-node: .github/workflows/rust.yml と同じ版を入れてください" >&2
  exit 1
fi

# 版が古ければ落とす。知らない設定を黙って無視されると「走ったが守られていない」が
# 作れてしまう（AGENTS.md「強制力の序列」のフェイルオープンかつサイレント）。
# cargo 側で nightly を要求しているのと同じ形。
installed_pnpm_version="$(pnpm --version)"
oldest_accepted="$(printf '%s\n%s\n' "$REQUIRED_PNPM_VERSION" "$installed_pnpm_version" | sort -V | head -1)"
if [ "$oldest_accepted" != "$REQUIRED_PNPM_VERSION" ]; then
  echo "resolve-node: pnpm $installed_pnpm_version は minimumReleaseAge を知りません" >&2
  echo "resolve-node: $REQUIRED_PNPM_VERSION 以降でないと cooldown が無視されるため、解決しません" >&2
  exit 1
fi

# **pnpm が読む値そのものを見る。** 設定ファイルを grep すると、値が 0（無効）でも、
# 名前がコメントにだけ残っていても通ってしまう。確かめたいのは書いてあることではなく
# 効いていることなので、pnpm に聞く。未設定なら `undefined` が返り、-gt が失敗する。
configured_cooldown="$(pnpm config get minimumReleaseAge 2>/dev/null || true)"
if ! [ "$configured_cooldown" -gt 0 ] 2>/dev/null; then
  echo "resolve-node: cooldown が効きません（minimumReleaseAge: ${configured_cooldown:-未設定}）" >&2
  echo "resolve-node: pnpm-workspace.yaml に分単位の正の値を書いてください" >&2
  exit 1
fi

echo "resolve-node: pnpm $*（pnpm $installed_pnpm_version / cooldown $configured_cooldown 分）"
exec pnpm "$@"
