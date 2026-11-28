#!/usr/bin/env bash
#
# .claude/hooks/plan-completion-check.sh のテスト。stdin の JSON から end-to-end で走らせる。
#
# 使い方: bash .claude/hooks/plan-completion-check-test.sh
set -euo pipefail

hook="$(cd "$(dirname "$0")" && pwd)/plan-completion-check.sh"
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

expect_exit "確かめ方の無い計画は止める" 2 "$(printf '## 計画\n\n- 完了条件: grep で確かめる\n')"
expect_exit "見出しの階層が違っても止める" 2 "$(printf '### 実装の計画\n\n- 腕を足す\n')"
expect_exit "コマンドと期待する出力があれば通す" 0 "$(printf '## 計画\n\n完了条件の確かめ方: git grep -n Foo src tests → 3 件\n')"
expect_exit "矢印の無い確かめ方は止める" 2 "$(printf '## 計画\n\n完了条件の確かめ方: CI を数回走らせる\n')"
expect_exit "期待する出力が空の確かめ方は止める" 2 "$(printf '## 計画\n\n完了条件の確かめ方: cargo test →\n')"
expect_exit "コマンドが空の確かめ方は止める" 2 "$(printf '## 計画\n\n完了条件の確かめ方: → 3 件\n')"
expect_exit "形の崩れた行が 1 つでもあれば止める" 2 "$(printf '## 計画\n\n完了条件の確かめ方: git grep -n Foo src → 3 件\n完了条件の確かめ方: テストが通る\n')"
expect_exit "0 件しか待たない確かめ方は止める" 2 "$(printf '## 計画\n\n完了条件の確かめ方: git grep -n Old src → 0 件\n')"
expect_exit "全角の 0 件しか待たない確かめ方も止める" 2 "$(printf '## 計画\n\n完了条件の確かめ方: git grep -n Old src → ０件\n')"
expect_exit "0 件の行と在ることを数える行が並べば通す" 0 "$(printf '## 計画\n\n完了条件の確かめ方: git grep -n Old src → 0 件\n完了条件の確かめ方: git grep -n New src → 2 件\n')"
expect_exit "10 件は 0 件に読まない" 0 "$(printf '## 計画\n\n完了条件の確かめ方: git grep -n New src → 10 件\n')"
expect_exit "テストの名前を待つ確かめ方は通す" 0 "$(printf '## 計画\n\n完了条件の確かめ方: cargo test test_alias_opens → 1 passed\n')"
expect_exit "箇条書きと太字の確かめ方も通す" 0 "$(printf '## 計画\n\n- **完了条件の確かめ方**: なし（コメントだけの変更）\n')"
expect_exit "中身の無い確かめ方の行は止める" 2 "$(printf '## 計画\n\n完了条件の確かめ方:\n')"
expect_exit "行の頭でない確かめ方は止める" 2 "$(printf '## 計画\n\n（完了条件の確かめ方: cargo test → ok）\n')"
expect_exit "見出しでない計画の語は通す" 0 "計画の検証で 2 件採りました"
expect_exit "計画の見出しが無い本文は通す" 0 "$(printf '## 却下した案\n\n- 案 A\n')"
expect_exit "本文が空なら通す" 0 ""

if [ "$failures" -ne 0 ]; then
  exit 1
fi
