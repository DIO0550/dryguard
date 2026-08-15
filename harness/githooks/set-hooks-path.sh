#!/usr/bin/env bash
#
# `core.hooksPath` を harness/githooks へ向ける。
#
# cargo には npm/pnpm の `prepare` に当たるフックが無いため、複数の入口から呼ぶ
# （DevContainer の postCreateCommand / Claude Code の SessionStart）。
# どこから呼ばれても結果は同じなので、重ねて呼んで構わない。
#
# 失敗しても呼び出し元を止めない。検査のための設定が、開発そのものを妨げないため。
set -uo pipefail

if ! command -v git >/dev/null 2>&1; then
  echo "githooks: git が無いため core.hooksPath の設定を飛ばします" >&2
  exit 0
fi

if ! git rev-parse --git-dir >/dev/null 2>&1; then
  echo "githooks: git リポジトリではないため core.hooksPath の設定を飛ばします" >&2
  exit 0
fi

if ! git config core.hooksPath harness/githooks; then
  echo "githooks: core.hooksPath の設定に失敗しました。手で 'git config core.hooksPath harness/githooks' を実行してください" >&2
  exit 0
fi

echo "githooks: core.hooksPath = harness/githooks"
