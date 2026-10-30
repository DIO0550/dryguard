#!/usr/bin/env bash
#
# PreToolUse（Issue コメント・Issue の本文を書く MCP ツール）で、計画の見出しを持つ本文に
# 影響範囲を grep で挙げた行が無ければ投稿を止める（層 3）。
#
# `plan` の分類で最も多いのは、計画が「変更が及ぶ既存のもの」（文書・既存テスト・コメント）を
# 挙げ漏れた形で、`implementation-flow` の観点（層 4）が書いてあっても再発している。
# ここは**投稿の前に一度立ち止まらせる**だけで、挙げた範囲が足りているかは見ない。
# 発火しない環境では、止まる機会が無くなるだけ。
#
# 計画の見出し（`## 計画` など、行頭の `#` と「計画」を含む行）を持つ本文には、
# 影響範囲を grep した行を 1 行求める:
#   影響範囲の出どころ: grep -rn 'typescript' README.md docs rules src → 14 箇所
# 影響範囲が無い計画は「影響範囲の出どころ: なし（理由）」と書く。
#
# 終了コード: 計画の見出しがあって出どころの行が無ければ 2（stderr を Claude に返す）。それ以外は 0。
set -uo pipefail

if ! command -v jq >/dev/null 2>&1; then
  exit 0
fi

body="$(jq -r '.tool_input.body // empty' 2>/dev/null)"

# 見出しの行だけを計画の印と読む。本文中の「計画」という語（「計画の検証」への言及など）は当たらない。
# Why not（「計画」の語をすべて印と読む）: 判断の履歴を書くコメントまで毎回止めることになる。
plan_heading_pattern='^#{1,6}[[:space:]].*計画'
source_pattern='^[[:space:]]*([-*+][[:space:]]+)?(\*\*)?影響範囲の出どころ(\*\*)?[：:][[:space:]]*[^[:space:]]'

if ! printf '%s\n' "$body" | grep -Eq "$plan_heading_pattern"; then
  exit 0
fi

if printf '%s\n' "$body" | grep -Eq "$source_pattern"; then
  exit 0
fi

{
  echo "plan-impact-check: 計画の見出しがありますが、影響範囲の出どころの行がありません。"
  echo "変える語・型・関数を README.md / docs / rules / src / tests で grep し、本文に 1 行足してください。"
  echo "    影響範囲の出どころ: <走らせたコマンド（対象の範囲を決めたもの）> → <出力>"
  echo "影響範囲が無ければ「影響範囲の出どころ: なし（理由）」と書いてください。"
} >&2
exit 2
