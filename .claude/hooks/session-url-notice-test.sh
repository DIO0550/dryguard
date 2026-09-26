#!/usr/bin/env bash
#
# .claude/hooks/session-url-notice.sh のテスト。
#
# フックを end-to-end で走らせ、終了コードと stdout を突き合わせる。セッションの ID は
# 環境変数で、ブランチは使い捨ての git リポジトリで作る（本物のセッション・作業ツリーに
# 結果を左右させないため）。
#
# 使い方: bash .claude/hooks/session-url-notice-test.sh
set -euo pipefail

hook="$(cd "$(dirname "$0")" && pwd)/session-url-notice.sh"
bash_path="$(command -v bash)"
git_path="$(command -v git)"
workspace="$(mktemp -d)"
trap 'rm -rf "$workspace"' EXIT

failures=0

# 渡したブランチ名を HEAD に持つ git リポジトリを作り、そのパスを返す。
# コミットは作らない。生まれる前のブランチでも名前を読めることを併せて確かめるため。
repo_on() {
  local dir="${workspace}/repo-${1//\//-}"
  "$git_path" init -q -b "$1" "$dir"
  printf '%s' "$dir"
}

not_a_repo="${workspace}/plain"
mkdir -p "$not_a_repo"

# 何も置かない PATH。git が無い環境を作る。
empty_path="${workspace}/empty-path"
mkdir -p "$empty_path"

# 期待する終了コードと、stdout に出るべき文言・出てはいけない文言を突き合わせる。
# stdout だけを受ける。SessionStart が Claude のコンテキストに入れるのは stdout なので。
# 第 5 引数に空文字を渡すと CLAUDE_CODE_REMOTE_SESSION_ID を未設定にする。
expect_result() {
  local name="$1"
  local expected_message="$2"
  local unexpected_message="$3"
  local project_dir="$4"
  local session_id="$5"
  local path="${6:-$PATH}"

  local actual=0
  local output
  if [ -n "$session_id" ]; then
    output="$(printf '{}' | env -u CLAUDE_CODE_REMOTE_SESSION_ID PATH="$path" \
      CLAUDE_PROJECT_DIR="$project_dir" CLAUDE_CODE_REMOTE_SESSION_ID="$session_id" \
      "$bash_path" "$hook" 2>/dev/null)" || actual=$?
  else
    output="$(printf '{}' | env -u CLAUDE_CODE_REMOTE_SESSION_ID PATH="$path" \
      CLAUDE_PROJECT_DIR="$project_dir" \
      "$bash_path" "$hook" 2>/dev/null)" || actual=$?
  fi

  local reason=""
  if [ "$actual" -ne 0 ]; then
    reason="終了コードが 0 ではなく ${actual}"
  elif [ -n "$expected_message" ] && ! printf '%s' "$output" | grep -qF -- "$expected_message"; then
    reason="stdout に「${expected_message}」が無い"
  elif [ -n "$unexpected_message" ] && printf '%s' "$output" | grep -qF -- "$unexpected_message"; then
    reason="stdout に「${unexpected_message}」が出た"
  fi

  if [ -z "$reason" ]; then
    echo "ok   ${name}"
    return 0
  fi

  echo "FAIL ${name}: ${reason}"
  printf '%s\n' "$output" | sed 's/^/     | /'
  failures=$((failures + 1))
}

plain_branch="$(repo_on claude/lucid-johnson-thx4nd)"
issue_branch="$(repo_on claude/issue-51-session-url)"

url="https://claude.ai/code/session_01ABCdef234"
cannot_build="URL を組み立てられません"

# --- URL を組み立てる ---
expect_result "cse_ の接頭辞を session_ に替えて URL にする" \
  "$url" "$cannot_build" "$plain_branch" "cse_01ABCdef234"

# --- URL を組み立てない（対照: 同じリポジトリで組み立てるのは上の cse_ の形だけ） ---
expect_result "ID が無ければ URL を出さず、組み立てられないと出す" \
  "$cannot_build" "https://claude.ai/code/" "$plain_branch" ""

expect_result "cse_ で始まらない ID からは URL を組み立てない" \
  "$cannot_build" "https://claude.ai/code/" "$plain_branch" "953124d9-4976-50dc-95b7-a17635ddb682"

expect_result "接頭辞だけで中身の無い ID からは URL を組み立てない" \
  "$cannot_build" "https://claude.ai/code/" "$plain_branch" "cse_"

expect_result "英数字以外を含む ID からは URL を組み立てない" \
  "$cannot_build" "https://claude.ai/code/" "$plain_branch" "cse_01ABC/../x"

# --- Issue 番号を添える ---
expect_result "claude/issue-<N>- のブランチなら Issue 番号を添える" \
  "Issue #51" "" "$issue_branch" "cse_01ABCdef234"

expect_result "issue-<N> で終わるブランチにも Issue 番号を添える" \
  "Issue #51" "" "$(repo_on claude/issue-51)" "cse_01ABCdef234"

expect_result "ID が無くてもブランチの Issue 番号は添える" \
  "Issue #51" "https://claude.ai/code/" "$issue_branch" ""

# --- Issue 番号を添えない（対照: 同じ ID で添えるのは上の issue ブランチだけ） ---
expect_result "claude/issue- で始まらないブランチには Issue 番号を添えない" \
  "$url" "Issue #" "$plain_branch" "cse_01ABCdef234"

expect_result "issue- の後ろが数字でないブランチには Issue 番号を添えない" \
  "$url" "Issue #" "$(repo_on claude/issue-abc-x)" "cse_01ABCdef234"

expect_result "issue-<N> の後ろに区切りが無いブランチには Issue 番号を添えない" \
  "$url" "Issue #" "$(repo_on claude/issue-51x)" "cse_01ABCdef234"

expect_result "git の作業ツリーでなければ Issue 番号を添えず、URL は出す" \
  "$url" "Issue #" "$not_a_repo" "cse_01ABCdef234"

expect_result "git が無くても URL は出す" \
  "$url" "Issue #" "$issue_branch" "cse_01ABCdef234" "$empty_path"

if [ "$failures" -ne 0 ]; then
  echo "${failures} 件のテストが落ちました"
  exit 1
fi
echo "すべて通りました"
