#!/usr/bin/env bash
#
# PreToolUse（Issue コメント・Issue の本文を書く MCP ツール）で、計画の見出しを持つ本文に
# 影響範囲を grep で挙げた行が無い、または標準の置き場所を対象にしていなければ投稿を止める（層 3）。
#
# `plan` の分類で最も多いのは、計画が「変更が及ぶ既存のもの」（文書・既存テスト・コメント）を
# 挙げ漏れた形で、`implementation-flow` の観点（層 4）が書いてあっても再発している。
# ここは**投稿の前に一度立ち止まらせる**だけで、挙げた範囲が足りているかは見ない。
# 発火しない環境では、止まる機会が無くなるだけ。
#
# 計画の見出し（`## 計画` など、行頭の `#` と「計画」を含む行）を持つ本文には、
# 影響範囲を grep した行を 1 行求める:
#   影響範囲の出どころ: grep -rn 'typescript' README.md AGENTS.md docs rules src tests → 14 箇所
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

source_line="$(printf '%s\n' "$body" | grep -E "$source_pattern" | head -1)"

if [ -n "$source_line" ]; then
  # 「なし（理由）」は grep する対象が無いと宣言した行なので、範囲は求めない。
  if printf '%s\n' "$source_line" | grep -Eq '[：:][[:space:]]*なし'; then
    exit 0
  fi
  # 規約の文書・コード内の doc・既存テストはどれも挙げ漏れの実績がある（pr-480〜pr-494 の
  # `plan` で、AGENTS.md・docs・rules・doc コメントの列挙漏れが繰り返された）。
  # grep の対象に標準の置き場所がすべて入っているかだけを見る。
  # Why not（置き場所ごとの 0 件も求める）: 出力の書き方まで決めると儀式になる。
  missing=""
  for root in README.md AGENTS.md docs rules src tests; do
    case "$source_line" in
      *"$root"*) ;;
      *) missing="$missing $root" ;;
    esac
  done
  # 同じ事実の言い回しは 1 つとは限らない（pr-494 指摘 5 / 6・pr-502 指摘 22 は、grep した語に
  # 当たらない言い回しの箇所が漏れた）。コマンド側（`→` より前）に言い回しを 2 つ以上
  # 並べた形（`a|b` か `-e a -e b`）かだけを見る。語が足りているかは見ない。
  # Why not（語の数や中身まで見る）: 足りているかは機械で言えず、数だけ合わせる儀式になる。
  command_part="${source_line%%→*}"
  if [ -z "$missing" ]; then
    case "$command_part" in
      *"|"*) exit 0 ;;
      *" -e "*" -e "*) exit 0 ;;
    esac
    {
      echo "plan-impact-check: 影響範囲の出どころの行が、言い回しを 1 つしか grep していません。"
      echo "変える事実の別の言い回し（例: 「未実測」と「まだ無い」）を '語A|語B' か -e を重ねて足し、grep し直してください。"
    } >&2
    exit 2
  fi
  {
    echo "plan-impact-check: 影響範囲の出どころの行が、grep の対象に次を含んでいません:$missing"
    echo "標準の置き場所（README.md AGENTS.md docs rules src tests）をすべて対象に grep し直してください。"
    echo "対象外にする置き場所があれば、その行を分けず、コマンドに置き場所を残して 0 件の出力を書いてください。"
  } >&2
  exit 2
fi

{
  echo "plan-impact-check: 計画の見出しがありますが、影響範囲の出どころの行がありません。"
  echo "変える語・型・関数を README.md / docs / rules / src / tests で grep し、本文に 1 行足してください。"
  echo "    影響範囲の出どころ: <走らせたコマンド（対象の範囲を決めたもの）> → <出力>"
  echo "影響範囲が無ければ「影響範囲の出どころ: なし（理由）」と書いてください。"
} >&2
exit 2
