#!/usr/bin/env bash
#
# 分類ごとの「最後の介入以降の再発数」を数える。
#
# 読むのは記録の次の 2 種類の行:
#   - 分類: `<分類>`                       … 指摘 1 件
#   - 対策済: `<分類>` 層=<層> at pr-<番号> … その回に介入したこと
#
# 出力の列:
#   再発      最後の介入より後の記録に出た件数。介入するかどうかはこれで判断する
#   通算      全記録での件数。語彙が飽和していないかを目視するときの参考
#   以降      最後の介入より後の記録の本数（「介入後 N 本再発ゼロ」の N）
#   最終介入  最後に置いた層と、その回の PR 番号。無ければ「未介入」
set -euo pipefail

cd "$(dirname "$0")"

# 記録が 1 本も無い状態から始まるので、glob が展開されない場合を先に返す。
# ここを踏まないと `pr-*.md` という名前のファイルを grep しに行って落ちる。
shopt -s nullglob
records=(pr-*.md)
shopt -u nullglob

if [ "${#records[@]}" -eq 0 ]; then
  echo "記録がまだ 1 本もありません。マージのたびに pr-<番号>.md を 1 ファイル増やしてください。"
  exit 0
fi

tags="$(grep -h '^- 分類: `' "${records[@]}" | sed 's/^- 分類: `\([^`]*\)`.*/\1/' | sort -u)"

body=""
for tag in $tags; do
  # 同じ分類の対策済が複数あれば、PR 番号が最大のものが最後の介入
  last="$( { grep -hoE "^- 対策済: \`$tag\` 層=[^ ]+ at pr-[0-9]+" "${records[@]}" || true; } \
    | sed 's/.*層=\([^ ]*\) at pr-\([0-9]*\)/\2 \1/' | sort -n | tail -1)"
  if [ -n "$last" ]; then
    last_pr="${last%% *}"
    intervention="pr-${last_pr}（層=${last##* }）"
  else
    last_pr=0
    intervention="未介入"
  fi

  recurrence=0
  total=0
  after=0
  for record in "${records[@]}"; do
    number="${record#pr-}"
    number="${number%.md}"
    count="$(grep -c "^- 分類: \`$tag\`" "$record" || true)"
    total=$((total + count))
    [ "$number" -gt "$last_pr" ] || continue
    recurrence=$((recurrence + count))
    after=$((after + 1))
  done

  body="${body}$(printf '%4d  %4d  %4d  %-20s %s' \
    "$recurrence" "$total" "$after" "$tag" "$intervention")
"
done

printf '%s\n' "再発  通算  以降  分類                 最終介入"
printf '%s' "$body" | sort -rn
