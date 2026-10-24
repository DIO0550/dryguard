#!/usr/bin/env bash
# send.sh の割り方のテスト。DISCORD_DRY_RUN=1 で送らずに、割った 1 通ごとの文字数を見る。
set -u
cd "$(dirname "$0")"
fail=0

run() { printf '%s' "$1" | DISCORD_DRY_RUN=1 bash ./send.sh; }
expect() { # <名前> <期待する出力> <実際の出力>
  if [ "$2" = "$3" ]; then echo "ok: $1"; else echo "FAIL: $1 (期待 '$2' / 実際 '$3')"; fail=1; fi
}

short="$(printf 'a%.0s' $(seq 1 100))"
expect "上限に収まる本文は 1 通で送る" "chunk 1/1: 100" "$(run "$short")"

line="$(printf 'あ%.0s' $(seq 1 1000))"
expect "2000 文字を超える本文は行の境界で割る（文字数で数える）" \
  $'chunk 1/2: 1006\nchunk 2/2: 1006' "$(run "$line"$'\n'"$line")"

long="$(printf 'b%.0s' $(seq 1 2500))"
expect "1 行が上限を超えるときだけ行の途中で切る" \
  $'chunk 1/2: 1906\nchunk 2/2: 606' "$(run "$long")"

printf '' | DISCORD_DRY_RUN=1 bash ./send.sh >/dev/null 2>&1
expect "空の本文は送らず失敗する" 1 "$?"
exit "$fail"
