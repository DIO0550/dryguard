#!/usr/bin/env bash
# Discord の webhook へ、標準入力の本文を上限内に割って送る。
#
# 使い方: printf '%s' "$CONTENT" | bash send.sh
# 環境変数: DISCORD_WEBHOOK_URL（送るとき）/ DISCORD_DRY_RUN=1（送らず、割った結果の長さを出す）
#
# 割る単位は行。1 行が上限を超えるときだけ行の途中で切る。
# 上限は 2000 文字だが、通知の頭に付ける `(i/n)` の分の余裕を持たせて 1900 にする。
set -euo pipefail
export LC_ALL=C.UTF-8

LIMIT=1900

content="$(cat)"
[ -n "$content" ] || { echo "本文が空です" >&2; exit 1; }

chunks=()
current=""
flush() {
  [ -n "$current" ] && chunks+=("$current")
  current=""
}
while IFS= read -r line || [ -n "$line" ]; do
  while [ "${#line}" -gt "$LIMIT" ]; do
    flush
    chunks+=("${line:0:LIMIT}")
    line="${line:LIMIT}"
  done
  if [ -z "$current" ]; then
    current="$line"
  elif [ $((${#current} + 1 + ${#line})) -le "$LIMIT" ]; then
    current+=$'\n'"$line"
  else
    flush
    current="$line"
  fi
done <<<"$content"
flush

total="${#chunks[@]}"
index=0
for chunk in "${chunks[@]}"; do
  index=$((index + 1))
  [ "$total" -gt 1 ] && chunk="($index/$total)"$'\n'"$chunk"
  if [ "${DISCORD_DRY_RUN:-}" = 1 ]; then
    echo "chunk $index/$total: ${#chunk}"
    continue
  fi
  : "${DISCORD_WEBHOOK_URL:?DISCORD_WEBHOOK_URL が未設定です}"
  status="$(curl -sS -o /dev/null -w '%{http_code}' -H 'Content-Type: application/json' \
    -d "$(jq -n --arg c "$chunk" '{content: $c}')" "$DISCORD_WEBHOOK_URL")"
  case "$status" in
    2??) ;;
    *) echo "Discord が $index/$total 通目に http $status を返しました" >&2; exit 1 ;;
  esac
done
