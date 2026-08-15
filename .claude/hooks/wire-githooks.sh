#!/usr/bin/env bash
#
# SessionStart で `core.hooksPath` を harness/githooks へ向ける。
#
# cargo には npm/pnpm の `prepare` に当たるフックが無いため、配線する入口を重ねている
# （harness/githooks/README.md「配線 — cargo に `prepare` が無い」）。ここはその 1 つ。
#
# このフック自体が読まれない実行環境がある（AGENTS.md「強制力の序列」の層 3）。
# **配線されないまま push されることを前提に設計してある** ので、失敗しても穴は開かない
# ── 無条件のゲートは CI（層 1）にある。
set -uo pipefail

project_dir="${CLAUDE_PROJECT_DIR:-.}"
cd "$project_dir" || exit 0

bash harness/githooks/set-hooks-path.sh >/dev/null 2>&1 || true

exit 0
