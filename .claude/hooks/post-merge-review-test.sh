#!/usr/bin/env bash
#
# .claude/hooks/post-merge-review.sh のテスト。
#
# 前半はコマンドの判定（lib/gh-pr-merge-command.sh）を、後半はフックを stdin の JSON から
# end-to-end で走らせ、終了コードと stdout の JSON を突き合わせる。
#
# 使い方: bash .claude/hooks/post-merge-review-test.sh
set -euo pipefail

hooks_dir="$(cd "$(dirname "$0")" && pwd)"
hook="${hooks_dir}/post-merge-review.sh"
# shellcheck source=lib/gh-pr-merge-command.sh
source "${hooks_dir}/lib/gh-pr-merge-command.sh"

bash_path="$(command -v bash)"
workspace="$(mktemp -d)"
trap 'rm -rf "$workspace"' EXIT

# フックが使うコマンドのうち jq だけを置かない PATH。jq が無い環境を作る。
no_jq_path="${workspace}/no-jq-path"
mkdir -p "$no_jq_path"
for tool in dirname cat sed tr; do
  ln -s "$(command -v "$tool")" "${no_jq_path}/${tool}"
done

failures=0

report() {
  local name="$1"
  local reason="$2"
  local output="${3:-}"

  if [ -z "$reason" ]; then
    echo "ok   ${name}"
    return 0
  fi

  echo "FAIL ${name}: ${reason}"
  if [ -n "$output" ]; then
    printf '%s\n' "$output" | sed 's/^/     | /'
  fi
  failures=$((failures + 1))
}

# マージと読み、PR の番号が期待どおり（空なら「読めない」）かを確かめる。
expect_merge() {
  local name="$1"
  local command="$2"
  local expected_number="$3"
  local reason=""
  local number
  if ! number="$(merged_pull_request_of "$command")"; then
    reason="マージとして読まなかった: ${command}"
  elif [ "$number" != "$expected_number" ]; then
    reason="PR の番号が「${expected_number}」ではなく「${number}」: ${command}"
  fi
  report "$name" "$reason"
}

expect_not_merge() {
  local name="$1"
  local command="$2"
  local reason=""
  if merged_pull_request_of "$command" >/dev/null; then
    reason="マージとして読んだ: ${command}"
  fi
  report "$name" "$reason"
}

# --- マージと読む ---
expect_merge "番号を指定した gh pr merge に当たる" 'gh pr merge 52 --squash' "52"
expect_merge "PR の URL から番号を読む" 'gh pr merge https://github.com/DIO0550/dryguard/pull/250 --merge' "250"
expect_merge "&& の後ろの gh pr merge に当たる" 'cargo test && gh pr merge 52' "52"
expect_merge "環境変数の代入の後ろの gh pr merge に当たる" 'GH_REPO=DIO0550/dryguard gh pr merge 52' "52"
expect_merge "値を取るオプションの値を PR の指定と取り違えない" 'gh pr merge --subject "Merge 7" -R DIO0550/dryguard 52' "52"
expect_merge "--opt=値 の形のオプションを飛ばして番号を読む" 'gh pr merge --repo=DIO0550/dryguard 52' "52"
expect_merge "PR を指定しない gh pr merge は、番号を読めないまま当たる" 'gh pr merge --squash' ""
expect_merge "ブランチ名で指定した gh pr merge は、番号を読めないまま当たる" 'gh pr merge claude/issue-52' ""

# --- マージと読まない（対照: 同じ形で当たるのは上の gh pr merge だけ） ---
expect_not_merge "素の git merge には当たらない" 'git merge origin/main'
expect_not_merge "gh pr の別のサブコマンドには当たらない" 'gh pr view 52'
expect_not_merge "--auto で auto-merge を予約するだけの gh pr merge には当たらない" 'gh pr merge 52 --auto --squash'
expect_not_merge "--disable-auto で auto-merge を解除する gh pr merge には当たらない" 'gh pr merge 52 --disable-auto'
expect_not_merge "コミットメッセージの中の gh pr merge には当たらない" 'git commit -m "then gh pr merge 52"'
expect_not_merge "echo の引数の gh pr merge には当たらない" 'echo gh pr merge 52'

# フックを走らせ、終了コードが 0 で、stdout の additionalContext に期待する文言が
# 含まれるか（期待が空なら stdout が空か）を確かめる。
expect_hook() {
  local name="$1"
  local input="$2"
  local expected_message="$3"
  local path="${4:-$PATH}"

  local actual=0
  local output
  output="$(printf '%s' "$input" | env PATH="$path" "$bash_path" "$hook" 2>/dev/null)" || actual=$?

  local reason=""
  if [ "$actual" -ne 0 ]; then
    reason="終了コードが 0 ではなく ${actual}"
  elif [ -z "$expected_message" ]; then
    if [ -n "$output" ]; then
      reason="対象外なのに stdout が空でない"
    fi
  else
    local event context
    event="$(printf '%s' "$output" | jq -r '.hookSpecificOutput.hookEventName // empty' 2>/dev/null)" || event=""
    context="$(printf '%s' "$output" | jq -r '.hookSpecificOutput.additionalContext // empty' 2>/dev/null)" || context=""
    if [ "$event" != "PostToolUse" ]; then
      reason="hookSpecificOutput.hookEventName が PostToolUse でない"
    elif ! printf '%s' "$context" | grep -qF -- "$expected_message"; then
      reason="additionalContext に「${expected_message}」が無い"
    fi
  fi
  report "$name" "$reason" "$output"
}

mcp_input() {
  printf '{"tool_name":"mcp__github__merge_pull_request","tool_input":{"owner":"DIO0550","repo":"dryguard","pullNumber":%s}}' "$1"
}

bash_input() {
  jq -cn --arg command "$1" '{tool_name: "Bash", tool_input: {command: $command}}'
}

# --- 促す ---
expect_hook "MCP のマージで PR と記録のファイルを示して促す" \
  "$(mcp_input 250)" "harness/records/pr-250.md"
expect_hook "MCP のマージでリポジトリ付きの PR を示す" \
  "$(mcp_input 250)" "PR DIO0550/dryguard#250"
expect_hook "MCP の番号が文字列でも読む" \
  "$(mcp_input '"251"')" "harness/records/pr-251.md"
expect_hook "MCP の番号が読めなくても、読めなかったと出して促す" \
  "$(mcp_input null)" "PR（番号を読めなかった）"
expect_hook "gh pr merge で PR と記録のファイルを示して促す" \
  "$(bash_input 'gh pr merge 52 --squash')" "harness/records/pr-52.md"
expect_hook "Issue への追記も促す" \
  "$(bash_input 'gh pr merge 52 --squash')" "関連 Issue へ"
expect_hook "番号の無い gh pr merge でも、読めなかったと出して促す" \
  "$(bash_input 'gh pr merge claude/issue-52')" "PR（番号を読めなかった）"

# --- 促さない（対照: 同じ Bash の入力で促すのは上の gh pr merge だけ） ---
expect_hook "素の git merge では促さない" "$(bash_input 'git merge origin/main')" ""
expect_hook "--auto の gh pr merge では促さない" "$(bash_input 'gh pr merge 52 --auto')" ""
expect_hook "auto-merge を有効にする MCP のツールでは促さない" \
  '{"tool_name":"mcp__github__enable_pr_auto_merge","tool_input":{"owner":"DIO0550","repo":"dryguard","pullNumber":52}}' ""
expect_hook "JSON でない入力は黙って通す" 'not json' ""
expect_hook "jq が無ければ黙って通す" "$(mcp_input 250)" "" "$no_jq_path"

if [ "$failures" -ne 0 ]; then
  echo "${failures} 件のテストが落ちました"
  exit 1
fi
echo "すべて通りました"
