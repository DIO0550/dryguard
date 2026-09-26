#!/usr/bin/env bash
#
# .claude/hooks/hook-canary.sh のテスト。
#
# フックを stdin の JSON から end-to-end で走らせ、終了コードと stderr を突き合わせる。
# PATH は使い捨てのディレクトリだけにし、そこに置いた偽のコマンドで jq / cargo の
# 有無を作る（本物の有無に結果を左右させないため）。
#
# 使い方: bash .claude/hooks/hook-canary-test.sh
set -euo pipefail

hook="$(cd "$(dirname "$0")" && pwd)/hook-canary.sh"
bash_path="$(command -v bash)"
workspace="$(mktemp -d)"
trap 'rm -rf "$workspace"' EXIT

failures=0

# 名前を渡した分だけ、何もしない偽のコマンドを置いた PATH 用のディレクトリを作る。
path_with() {
  local dir="${workspace}/path-$(printf '%s-' "$@")"
  mkdir -p "$dir"
  local tool
  for tool in "$@"; do
    printf '#!/bin/sh\nexit 0\n' > "${dir}/${tool}"
    chmod +x "${dir}/${tool}"
  done
  printf '%s' "$dir"
}

# PreToolUse が stdin に渡す JSON のうち、フックが読む部分だけを手で組み立てる。
# Why not（jq -n で組み立てる）: テスト自身が jq を要すると、jq の無い環境を作れない。
payload_of() {
  printf '{"tool_name":"Bash","tool_input":{"command":"%s"}}' "$1"
}

# 期待する終了コードと、stderr に出るべき文言・出てはいけない文言を突き合わせる。
# stderr だけを受ける。PreToolUse が exit 2 で Claude に返すのは stderr だけなので。
expect_result() {
  local name="$1"
  local expected="$2"
  local expected_message="$3"
  local unexpected_message="$4"
  local path="$5"
  local payload="$6"

  local actual=0
  local output
  output="$(printf '%s' "$payload" | PATH="$path" "$bash_path" "$hook" 2>&1 >/dev/null)" || actual=$?

  local reason=""
  if [ "$actual" -ne "$expected" ]; then
    reason="終了コードが ${expected} ではなく ${actual}"
  elif [ -n "$expected_message" ] && ! printf '%s' "$output" | grep -qF -- "$expected_message"; then
    reason="stderr に「${expected_message}」が無い"
  elif [ -n "$unexpected_message" ] && printf '%s' "$output" | grep -qF -- "$unexpected_message"; then
    reason="stderr に「${unexpected_message}」が出た"
  fi

  if [ -z "$reason" ]; then
    echo "ok   ${name}"
    return 0
  fi

  echo "FAIL ${name}: ${reason}"
  printf '%s\n' "$output" | sed 's/^/     | /'
  failures=$((failures + 1))
}

all_tools="$(path_with jq cargo)"
no_tools="$(path_with)"
only_cargo="$(path_with cargo)"

fired="フックが発火しています"

# --- 当てる ---
expect_result "echo hook-canary を止める" \
  2 "$fired" "" "$all_tools" "$(payload_of 'echo hook-canary')"

expect_result "キーと値の間に空白がある JSON でも止める" \
  2 "$fired" "" "$all_tools" \
  '{"tool_name": "Bash", "tool_input": {"command": "echo hook-canary", "description": "x"}}'

expect_result "PATH に何も無くても止める（判定を外部コマンドに頼らない）" \
  2 "$fired" "" "$no_tools" "$(payload_of 'echo hook-canary')"

# --- 当てない（対照: 同じ PATH で止めるのは上の echo hook-canary だけ） ---
expect_result "ほかのコマンドは通す" \
  0 "" "$fired" "$all_tools" "$(payload_of 'echo hello')"

expect_result "hook-canary を文字列として含むだけのコマンドは通す" \
  0 "" "$fired" "$all_tools" "$(payload_of 'git commit -m \"echo hook-canary\"')"

expect_result "echo hook-canary の後ろに続きがあるコマンドは通す" \
  0 "" "$fired" "$all_tools" "$(payload_of 'echo hook-canary && git push')"

expect_result "空の入力は通す" \
  0 "" "" "$all_tools" ''

# --- 欠けているコマンドの報告 ---
expect_result "jq と cargo が揃っていれば欠けているとは出さない" \
  2 "" "ただし次のコマンドが無く" "$all_tools" "$(payload_of 'echo hook-canary')"

expect_result "jq が無ければ名前を出す（cargo は出さない）" \
  2 "素通りします: jq" "cargo" "$only_cargo" "$(payload_of 'echo hook-canary')"

expect_result "どちらも無ければ両方の名前を出す" \
  2 "素通りします: jq cargo" "" "$no_tools" "$(payload_of 'echo hook-canary')"

if [ "$failures" -ne 0 ]; then
  echo "${failures} 件のテストが落ちました"
  exit 1
fi
echo "すべて通りました"
