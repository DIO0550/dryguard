#!/usr/bin/env bash
#
# .claude/hooks/issue-comment-count-check.sh のテスト。stdin の JSON から end-to-end で走らせる。
#
# 使い方: bash .claude/hooks/issue-comment-count-check-test.sh
set -euo pipefail

hook="$(cd "$(dirname "$0")" && pwd)/issue-comment-count-check.sh"
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

expect_exit "出どころの無い箇所数は止める" 2 "src の外に 3 箇所残っています"
expect_exit "出どころの無いファイル数は止める" 2 "7 ファイルを変えます"
expect_exit "数字と単位のあいだの空白が無くても止める" 2 "出現は70行です"
expect_exit "出どころの行があれば通す" 0 "$(printf 'src の外に 3 箇所\n\n数の出どころ: grep -rn Grammar src | wc -l → 3\n')"
expect_exit "箇条書きと太字の出どころも通す" 0 "$(printf '62 行\n- **数の出どころ**: 未確認\n')"
expect_exit "中身の無い出どころの行は止める" 2 "$(printf '62 行\n数の出どころ:\n')"
expect_exit "行の頭でない出どころは止める" 2 "62 行（数の出どころ: grep）"
expect_exit "単位の付かない Issue 番号と日付は通す" 0 "#403 を 2026-10-25 に閉じる"
expect_exit "本文が空なら通す" 0 ""

if [ "$failures" -ne 0 ]; then
  exit 1
fi
