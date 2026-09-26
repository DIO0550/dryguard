#!/usr/bin/env bash
#
# PreToolUse（Bash）で `git push` を見つけたら、push の前に検査を走らせる（層 3）。
#
# 検査の本体は harness/githooks/pre-push をそのまま呼ぶ。層 2 との違いは発火条件だけで、
# こちらは Bash のコマンドが実行される前に落ちた検査の出力を返せる。
#
# **これを enforcement として数えない**（AGENTS.md「強制力の序列」）。このフック自体が
# 読まれない実行環境があり、タイムアウトしても通る。ゲートは層 2 と層 1 にある。
#
# 終了コード: 検査が落ちたら 2（コマンドを止め、stderr を Claude に返す）。それ以外は 0。
set -uo pipefail

hooks_dir="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=lib/git-push-command.sh
source "${hooks_dir}/lib/git-push-command.sh"

# 入力が読めないときは黙って通す。検査できないことを理由に止めても、検査の質は上がらない
# （harness/githooks/pre-push が cargo の無い環境で通すのと同じ理由）。
if ! command -v jq >/dev/null 2>&1; then
  exit 0
fi

command="$(jq -r '.tool_input.command // empty' 2>/dev/null)" || exit 0
if [ -z "$command" ] || ! is_git_push_command "$command"; then
  exit 0
fi

cd "${CLAUDE_PROJECT_DIR:-.}" || exit 0
if ! git rev-parse --show-toplevel >/dev/null 2>&1; then
  exit 0
fi

# 出力は成否にかかわらず stderr へ流す。PreToolUse が Claude に返すのは exit 2 の stderr だけ。
if ! bash harness/githooks/pre-push >&2; then
  echo "pre-push-check: 検査が落ちたため push を止めました。上の出力を直してから push してください" >&2
  exit 2
fi

exit 0
