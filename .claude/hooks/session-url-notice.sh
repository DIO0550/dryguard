#!/usr/bin/env bash
#
# SessionStart でこのセッションの URL（`https://claude.ai/code/session_<id>`）を組み立てて出す。
# ブランチが `claude/issue-<N>-...` なら対象の Issue 番号も添える。
#
# AGENTS.md「Issue に紐づいて起動したら、セッションの URL を Issue に残す」の材料。
# **URL はセッションの中からしか作れない** ので、規約だけに置くと「残そうと思ったが
# URL が分からない」で止まる。SessionStart の stdout は Claude のコンテキストに入る。
#
# 見たいのは GitHub のコメントでリポジトリに痕跡が残らないため、層 2・層 1 へは上げられない。
# このフック自体が読まれない実行環境がある（AGENTS.md「強制力の序列」の層 3）。
#
# 終了コード: 常に 0（セッションの開始を止める理由は無い）。
set -u

# URL の ID は `CLAUDE_CODE_REMOTE_SESSION_ID` の `cse_<id>` から接頭辞を替えたもの。
#
# Why not（stdin の `session_id` / `CLAUDE_CODE_SESSION_ID` から作る）: どちらも
# ローカルの UUID で URL の ID とは別物。組み立てると存在しない URL を Issue に残すことになる。
remote_id_pattern='^cse_([A-Za-z0-9]+)$'
session_id="${CLAUDE_CODE_REMOTE_SESSION_ID:-}"

# `claude/issue-<N>` の後ろは `-` か終わり。`claude/issue-51x` を #51 と読まない。
issue_branch_pattern='^claude/issue-([0-9]+)(-|$)'

if [[ $session_id =~ $remote_id_pattern ]]; then
  printf '%s\n' "このセッションの URL: https://claude.ai/code/session_${BASH_REMATCH[1]}"
else
  # Why not（黙る）: 何も出ないと、フックが不発だったのか URL の無いセッションなのかを
  # 区別できず、URL を推測で書く余地が残る。
  printf '%s\n' "このセッションの URL を組み立てられません（CLAUDE_CODE_REMOTE_SESSION_ID が cse_<英数字> の形でない）。推測で URL を書かないこと"
fi

project_dir="${CLAUDE_PROJECT_DIR:-.}"
# 生まれる前のブランチ（コミットが 0 件）でも名前を返すので `rev-parse` ではなくこちら。
branch="$(git -C "$project_dir" symbolic-ref --short -q HEAD 2>/dev/null)" || branch=''

if [[ $branch =~ $issue_branch_pattern ]]; then
  printf '%s\n' "ブランチ ${branch} の対象: Issue #${BASH_REMATCH[1]}。着手した時点で、この Issue へセッションの URL をコメントする"
fi

exit 0
