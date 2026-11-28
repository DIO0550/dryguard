#!/usr/bin/env bash
#
# PreToolUse（Issue コメント・Issue の本文を書く MCP ツール）で、計画の見出しを持つ本文に
# 完了条件を「走らせるコマンド → 期待する出力」の形で書いた行が無ければ投稿を止める（層 3）。
#
# `plan-completion-criterion` は、計画の完了条件が機械的に確かめられる形になっていない形
# （grep のパターンと範囲が無い・テストの名前が無い・「数回」で回数が決まらない・
# 旧い綴りが 0 件になることしか見ない）。`implementation-flow` の「3. 計画」と
# `plan-review.md`（層 4）に観点が在っても、計画の検証で毎回直している（pr-497・pr-505・
# pr-512・pr-519・pr-537・pr-502 指摘 24）。ここは**投稿の前に、コマンドと期待する出力を
# 1 行に書かせる**だけで、その条件で足りているかは見ない。発火しない環境では、止まる機会が無くなるだけ。
#
# 計画の見出し（`## 計画` など、行頭の `#` と「計画」を含む行）を持つ本文には、次の形の行を求める:
#   完了条件の確かめ方: git grep -nE 'Foo|foo' README.md docs rules src tests → 0 件
#   完了条件の確かめ方: git grep -n 'Bar' src → 3 件
# 機械的に確かめる完了条件が無い計画は「完了条件の確かめ方: なし（理由）」と書く。
#
# 終了コード: 計画の見出しがあって条件を満たす行が無ければ 2（stderr を Claude に返す）。それ以外は 0。
set -uo pipefail

if ! command -v jq >/dev/null 2>&1; then
  exit 0
fi

body="$(jq -r '.tool_input.body // empty' 2>/dev/null)"

# 計画の見出しの読み方は plan-impact-check.sh と揃える。片方だけが止まる本文を作らないため。
plan_heading_pattern='^#{1,6}[[:space:]].*計画'
criterion_pattern='^[[:space:]]*([-*+][[:space:]]+)?(\*\*)?完了条件の確かめ方(\*\*)?[：:][[:space:]]*[^[:space:]]'

if ! printf '%s\n' "$body" | grep -Eq "$plan_heading_pattern"; then
  exit 0
fi

criterion_lines="$(printf '%s\n' "$body" | grep -E "$criterion_pattern")"

if [ -z "$criterion_lines" ]; then
  {
    echo "plan-completion-check: 計画の見出しがありますが、完了条件の確かめ方の行がありません。"
    echo "完了条件ごとに、走らせるコマンドと期待する出力を 1 行で書いてください。"
    echo "    完了条件の確かめ方: <コマンド（grep ならパターンと対象の範囲まで）> → <期待する出力>"
    echo "機械的に確かめる完了条件が無ければ「完了条件の確かめ方: なし（理由）」と書いてください。"
  } >&2
  exit 2
fi

# 「なし（理由）」は確かめるコマンドが無いと宣言した行なので、形は求めない。
if printf '%s\n' "$criterion_lines" | grep -Eq '[：:][[:space:]]*なし'; then
  exit 0
fi

# コマンドと期待する出力の両方が無い行は、何を走らせて何が出れば完了かを言えない
# （pr-497 指摘 4・pr-519 指摘 7: grep のパターン・テストの名前が無かった、pr-537 指摘 1: 「数回」）。
# Why not（コマンドの中身まで見る）: 足りているかは機械で言えず、語を並べるだけの儀式になる。
unformed=""
while IFS= read -r line; do
  value="${line#*完了条件の確かめ方}"
  value="${value#\*\*}"
  value="${value#[：:]}"
  command_part="${value%%→*}"
  expected_part="${value#*→}"
  has_command="$(printf '%s' "$command_part" | tr -d '[:space:]')"
  has_expected="$(printf '%s' "$expected_part" | tr -d '[:space:]')"
  case "$value" in
    *"→"*) ;;
    *) has_expected="" ;;
  esac
  if [ -z "$has_command" ] || [ -z "$has_expected" ]; then
    unformed="$unformed
    $line"
  fi
done <<< "$criterion_lines"

if [ -n "$unformed" ]; then
  {
    echo "plan-completion-check: 完了条件の確かめ方の行に、コマンドか期待する出力が欠けています:$unformed"
    echo "「<コマンド> → <期待する出力>」の形にしてください（「数回」「通る」ではなく、回数・件数・テストの名前を書く）。"
  } >&2
  exit 2
fi

# どの行も「0 件」だけを待つなら、旧い綴りを行ごと消しても通る（pr-505 指摘 6・pr-512 指摘 4）。
# 新しい綴り・テストが在ることを数える行が 1 つは要る。
# Why not（0 件の行ごとに対になる行を求める）: どの行が対かを機械で言えず、書き方を決める儀式になる。
nonzero_found=""
while IFS= read -r line; do
  expected_part="${line#*→}"
  trimmed="$(printf '%s' "$expected_part" | sed -E 's/^[[:space:]]+//')"
  if ! printf '%s' "$trimmed" | grep -Eq '^(0|０)[[:space:]]*(件|箇所|行|個)?([[:space:]]|[（(]|$)|^(出力なし|空)'; then
    nonzero_found="yes"
    break
  fi
done <<< "$criterion_lines"

if [ -z "$nonzero_found" ]; then
  {
    echo "plan-completion-check: 完了条件の確かめ方が、どれも 0 件（消えたこと）しか待っていません。"
    echo "旧い綴りを行ごと消しても通るので、新しい綴り・テストが在ることを数える行を足してください。"
    echo "    完了条件の確かめ方: git grep -n '<新しい綴り>' <範囲> → N 件"
  } >&2
  exit 2
fi

exit 0
