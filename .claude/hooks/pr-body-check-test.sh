#!/usr/bin/env bash
#
# .claude/hooks/pr-body-check.sh のテスト。stdin の JSON から end-to-end で走らせる。
#
# 使い方: bash .claude/hooks/pr-body-check-test.sh
set -euo pipefail

hook="$(cd "$(dirname "$0")" && pwd)/pr-body-check.sh"
failures=0

expect_exit() {
  local name="$1"
  local expected="$2"
  local body="$3"
  local actual=0
  jq -n --arg b "$body" '{tool_name:"mcp__github__create_pull_request",tool_input:{body:$b}}' \
    | bash "$hook" >/dev/null 2>&1 || actual=$?
  if [ "$actual" -eq "$expected" ]; then
    echo "ok   ${name}"
  else
    echo "FAIL ${name}: 終了コードが ${expected} ではなく ${actual}"
    failures=$((failures + 1))
  fi
}

expect_exit "仮の本文 PLACEHOLDER は止める" 2 "PLACEHOLDER"
expect_exit "本文が空なら止める" 2 ""
expect_exit "数の無い宣言だけの行は止める" 2 "差分規模: あとで書く"
expect_exit "行の頭でない宣言は止める" 2 "本文中の 差分規模: 3 ファイル / +1 -0 という例"
expect_exit "宣言があれば通す" 0 "$(printf '概要\n\n差分規模: 3 ファイル / +61 -0\n')"
expect_exit "箇条書きと太字の宣言も通す" 0 "- **差分規模**: 3 ファイル / +61 -0"

if [ "$failures" -ne 0 ]; then
  exit 1
fi
