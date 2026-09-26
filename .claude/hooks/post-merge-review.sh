#!/usr/bin/env bash
#
# PostToolUse で PR のマージを検知したら、マージ後の振り返り（Issue への追記と
# harness/records/ への記録）を additionalContext で促す（層 3）。
#
# 検知するのは `mcp__github__merge_pull_request` と Bash の `gh pr merge` だけ。
# **素の `git merge` は見ない** — ベースブランチの取り込みで日常的に走り、拾うと誤発火のほうが多い。
#
# **ブロックしない。** マージは人の判断で行われるので、記録が無いことを理由に止めても
# 記録の質は上がらない。このフック自体が読まれない実行環境がある（AGENTS.md「強制力の序列」）。
#
# 終了コード: 常に 0。
set -uo pipefail

hooks_dir="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=lib/gh-pr-merge-command.sh
source "${hooks_dir}/lib/gh-pr-merge-command.sh"

readonly MCP_MERGE_TOOL='mcp__github__merge_pull_request'

# Why（`jq` が無ければ黙って通す）: Bash の `command` は JSON の文字列でエスケープを含み、
# 組み込みの正規表現で読み下すと取りこぼしの形が増える。`jq` の欠けは hook-canary.sh が報告する。
if ! command -v jq >/dev/null 2>&1; then
  exit 0
fi

input="$(cat)"
tool_name="$(printf '%s' "$input" | jq -r '.tool_name // empty' 2>/dev/null)" || exit 0

# マージした PR を `<リポジトリ>#<番号>` / `#<番号>` の形で持つ。番号が読めなければ空。
pull_request=""
case "$tool_name" in
  "$MCP_MERGE_TOOL")
    pull_request="$(printf '%s' "$input" | jq -r '
      .tool_input as $in
      | if ($in.pullNumber | tostring | test("^[0-9]+$")) then
          (if ($in.owner | type) == "string" and ($in.repo | type) == "string"
            then "\($in.owner)/\($in.repo)" else "" end) + "#\($in.pullNumber)"
        else "" end' 2>/dev/null)" || exit 0
    ;;
  Bash)
    command="$(printf '%s' "$input" | jq -r '.tool_input.command // empty' 2>/dev/null)" || exit 0
    number="$(merged_pull_request_of "$command")" || exit 0
    if [ -n "$number" ]; then
      pull_request="#${number}"
    fi
    ;;
  *)
    exit 0
    ;;
esac

if [ -n "$pull_request" ]; then
  subject="PR ${pull_request}"
  record="harness/records/pr-${pull_request##*#}.md"
else
  # Why not（黙る）: ブランチ名で指定したマージも振り返りの対象。番号は PR から引き直せる。
  subject="PR（番号を読めなかった）"
  record="harness/records/pr-<番号>.md"
fi

context="${subject} のマージを検知しました。マージが成功していれば、振り返りを行う:
1. 関連 Issue へ、実装中の判断・却下した案・手戻りを追記する（AGENTS.md「実装の進め方」）
2. ${record} に評価を記録する（harness-record スキル。書き方は harness/records/README.md と TEMPLATE.md）。harness/records/ 配下だけを変える PR なら記録は作らない
このリポジトリ以外の PR なら、どちらも対象外。これは促すだけで、何も止めていない"

jq -n --arg context "$context" \
  '{hookSpecificOutput: {hookEventName: "PostToolUse", additionalContext: $context}}'
exit 0
