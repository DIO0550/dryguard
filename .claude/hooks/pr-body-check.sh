#!/usr/bin/env bash
#
# PreToolUse（mcp__github__create_pull_request）で、本文に差分規模の宣言が無い PR の作成を止める。
#
# 層 1（harness/ci/pr-body-diff.sh）は同じ欠けを PR の作成後に落とす。ここは**作る前**に
# 止めるだけで、数が実際の差分と合っているかは見ない（それは層 1 の仕事）。
# 発火しない環境では即時性だけが失われ、同じ欠けは層 1 が拾う。
#
# 終了コード: 宣言の行が無ければ 2（stderr を Claude に返す）。あるとき・読めないときは 0。
set -uo pipefail

# jq が無い環境では黙って通す（post-edit-rust.sh と同じ扱い）。
if ! command -v jq >/dev/null 2>&1; then
  exit 0
fi

body="$(jq -r '.tool_input.body // empty' 2>/dev/null)"

# Why not（`pr-body-diff.sh` を呼ぶ）: 基点と先端の rev をこのフックが決めることになり、
# rev が引けない環境の失敗を本文の欠けと取り違える。ここで見るのは行の有無だけ。
# 綴りは pr-body-diff.sh の `prefix_pattern` と揃える（片方だけ直したら食い違う）。
declaration_pattern='^[[:space:]]*([-*+][[:space:]]+)?(\*\*)?差分規模(\*\*)?[：:][[:space:]]*[0-9]'

if printf '%s\n' "$body" | grep -Eq "$declaration_pattern"; then
  exit 0
fi

{
  echo "pr-body-check: PR の本文に差分規模の宣言がありません。仮の本文で作らず、"
  echo "確かめた本文をそのまま渡してください（作成時の CI が落ちます）。"
  echo "    差分規模: <ファイル数> ファイル / +<追加行> -<削除行>"
} >&2
exit 2
