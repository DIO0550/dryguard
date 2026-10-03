#!/usr/bin/env bash
# commit-msg フックのテスト。拒否する形と、通す形（対照）を 1 件ずつ置く。
set -u
cd "$(dirname "$0")"
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
fail=0

check() { # <期待する終了コード> <名前> <メッセージ>
  printf '%s\n' "$3" > "$tmp"
  bash ./commit-msg "$tmp" >/dev/null 2>&1
  code=$?
  if [ "$code" -ne "$1" ]; then echo "FAIL: $2 (exit $code, 期待 $1)"; fail=1; else echo "ok: $2"; fi
}

check 1 "discussion の URL を拒否する" $'fix\n\nhttps://github.com/o/r/pull/1#discussion_r123456'
check 1 "issuecomment の URL を拒否する" $'fix\n\nhttps://github.com/o/r/issues/1#issuecomment-99'
check 1 "pullrequestreview の URL を拒否する" $'fix\n\nhttps://github.com/o/r/pull/1#pullrequestreview-5'
check 0 "PR 番号だけなら通す" $'fix\n\nPR #12 のレビュー指摘 1 に対応'
check 0 "git のコメント行は見ない" $'fix\n# https://github.com/o/r/pull/1#discussion_r1'
exit "$fail"
