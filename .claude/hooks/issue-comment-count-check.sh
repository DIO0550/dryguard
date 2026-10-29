#!/usr/bin/env bash
#
# PreToolUse（Issue コメント・Issue の本文を書く MCP ツール）で、件数・箇所数を書いたのに
# 数えたコマンドを残していない本文の投稿を止める（層 3）。
#
# 層 1・層 2 は Issue コメントを見ない（PR 本文の差分規模だけが層 1 で突き合わされる）。
# 「N 件」「N 箇所」を数えずに・数える範囲を取り違えて書いた形は、Issue コメントとコミット本文に
# 繰り返し出ている（`unverified-claim`）。ここは**投稿の前に一度立ち止まらせる**だけで、
# 数が正しいかは見ない。発火しない環境では、止まる機会が無くなるだけ。
#
# 数を含む本文には、数えたコマンドと範囲を書いた行を 1 行求める:
#   数の出どころ: grep -rn 'Grammar::Rust' src | wc -l → 62
# 「未確認」と書いてもよい（確かめていないことを、確かめたように書かないための行）。
#
# 終了コード: 数があって出どころの行が無ければ 2（stderr を Claude に返す）。それ以外は 0。
set -uo pipefail

# jq が無い環境では黙って通す（pr-body-check.sh と同じ扱い）。
if ! command -v jq >/dev/null 2>&1; then
  exit 0
fi

body="$(jq -r '.tool_input.body // empty' 2>/dev/null)"

# 数と単位が隣り合う形だけを数の主張と読む。Issue 番号（#403）・日付・PR 番号は単位が付かないので当たらない。
# Why not（数字をすべて数の主張と読む）: 番号・版・日付まで止めて、毎回出どころの行を書かせることになる。
count_pattern='[0-9]+[[:space:]]*(件|箇所|か所|ファイル|行|種|個|回|本|つ)'
source_pattern='^[[:space:]]*([-*+][[:space:]]+)?(\*\*)?数の出どころ(\*\*)?[：:][[:space:]]*[^[:space:]]'

if ! printf '%s\n' "$body" | grep -Eq "$count_pattern"; then
  exit 0
fi

if printf '%s\n' "$body" | grep -Eq "$source_pattern"; then
  exit 0
fi

{
  echo "issue-comment-count-check: 本文に件数・箇所数がありますが、数の出どころの行がありません。"
  echo "書く直前に数え直したコマンドとその出力を見て、本文に 1 行足してください。"
  echo "    数の出どころ: <走らせたコマンド（範囲を決めたもの）> → <出力>"
  echo "数えていなければ「数の出どころ: 未確認」と書き、数の隣にも未確認と残してください。"
} >&2
exit 2
