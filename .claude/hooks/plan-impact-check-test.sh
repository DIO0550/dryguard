#!/usr/bin/env bash
#
# .claude/hooks/plan-impact-check.sh のテスト。stdin の JSON から end-to-end で走らせる。
#
# 使い方: bash .claude/hooks/plan-impact-check-test.sh
set -euo pipefail

hook="$(cd "$(dirname "$0")" && pwd)/plan-impact-check.sh"
failures=0

expect_exit() {
  local name="$1"
  local expected="$2"
  local body="$3"
  local actual=0
  jq -n --arg b "$body" '{tool_name:"mcp__github__add_issue_comment",tool_input:{body:$b}}' \
    | bash "$hook" >/dev/null 2>&1 || actual=$?
  if [ "$actual" -eq "$expected" ]; then
    echo "ok   ${name}"
  else
    echo "FAIL ${name}: 終了コードが ${expected} ではなく ${actual}"
    failures=$((failures + 1))
  fi
}

expect_exit "出どころの無い計画は止める" 2 "$(printf '## 計画\n\n- Rust の腕を足す\n')"
expect_exit "見出しの階層が違っても止める" 2 "$(printf '### 実装の計画\n\n- 腕を足す\n')"
expect_exit "出どころの行があれば通す" 0 "$(printf '## 計画\n\n影響範囲の出どころ: grep -rn typescript README.md → 3 箇所\n')"
expect_exit "箇条書きと太字の出どころも通す" 0 "$(printf '## 計画\n\n- **影響範囲の出どころ**: なし（コメントだけの変更）\n')"
expect_exit "中身の無い出どころの行は止める" 2 "$(printf '## 計画\n\n影響範囲の出どころ:\n')"
expect_exit "行の頭でない出どころは止める" 2 "$(printf '## 計画\n\n（影響範囲の出どころ: grep）\n')"
expect_exit "見出しでない計画の語は通す" 0 "計画の検証で 2 件採りました"
expect_exit "計画の見出しが無い本文は通す" 0 "$(printf '## 却下した案\n\n- 案 A\n')"
expect_exit "本文が空なら通す" 0 ""

if [ "$failures" -ne 0 ]; then
  exit 1
fi
