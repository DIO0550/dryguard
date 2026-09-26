#!/usr/bin/env bash
#
# Bash のコマンド文字列が `gh pr merge` を含むかを判定し、マージする PR の番号を読む。source して使う。
#
# 読み損ねた形は「マージではない」側へ倒す。取りこぼしても振り返りを促す文が出ないだけだが、
# マージでないコマンドをマージと読むと、まだ続いている PR の記録を書き始めさせてしまう
# （rules/coding.md「列挙で判定を組むときは、漏れの倒れる向きを選ぶ」）。

# shellcheck source=command-segments.sh
source "$(dirname "${BASH_SOURCE[0]}")/command-segments.sh"

# 値を 1 つ取る `gh pr merge` のオプション。値を位置引数（PR の指定）と取り違えないために飛ばす。
# 漏れると値が PR の指定に見えるが、数字でも `/pull/<N>` でもなければ番号が読めないだけで済む。
readonly GH_PR_MERGE_VALUE_OPTIONS=' -t --subject -b --body -F --body-file -R --repo -A --author-email --match-head-commit '

# コマンド文字列に `gh pr merge` があれば 0 を返し、PR の番号を出す（読めなければ空行）。
# 無ければ 1 を返し、何も出さない。
#
# 区切った 1 つずつ（lib/command-segments.sh）の先頭を見る。最初に当たったものだけを答える。
#
# Why not（部分文字列 `gh pr merge` を探す）: `git commit -m "then gh pr merge"` や
# `echo gh pr merge` までマージと読む。
merged_pull_request_of() {
  local segment
  while IFS= read -r segment; do
    if merged_pull_request_of_segment "$segment"; then
      return 0
    fi
  done < <(command_segments_of "$1")

  return 1
}

# 区切った 1 つ分のコマンドが `gh pr merge` なら 0 を返し、PR の番号を出す（読めなければ空行）。
#
# 先頭の環境変数の代入を飛ばした次が `gh pr merge` かを見る。PR の指定は数字か
# `.../pull/<N>` の URL のときだけ番号として読む（ブランチ名からは番号が決まらない）。
#
# `--auto` / `--disable-auto` は当てない。auto-merge を予約・解除するだけで、マージは
# その場では起きない。促すと、レビューが終わる前に記録を書き始めることになる。
merged_pull_request_of_segment() {
  local words
  read -ra words <<< "$1"

  local i=0
  local count="${#words[@]}"
  while [ "$i" -lt "$count" ] && [[ "${words[$i]}" =~ ^[A-Za-z_][A-Za-z0-9_]*= ]]; do
    i=$((i + 1))
  done

  if [ "$((i + 3))" -gt "$count" ] \
    || [ "${words[$i]}" != "gh" ] \
    || [ "${words[$((i + 1))]}" != "pr" ] \
    || [ "${words[$((i + 2))]}" != "merge" ]; then
    return 1
  fi
  i=$((i + 3))

  local target=""
  local target_seen=0
  while [ "$i" -lt "$count" ]; do
    local word="${words[$i]}"
    case "$word" in
      --auto | --disable-auto) return 1 ;;
      -*)
        if [[ $GH_PR_MERGE_VALUE_OPTIONS == *" ${word} "* ]]; then
          i=$((i + 2))
        else
          i=$((i + 1))
        fi
        ;;
      *)
        if [ "$target_seen" -eq 0 ]; then
          target="$word"
          target_seen=1
        fi
        i=$((i + 1))
        ;;
    esac
  done

  if [[ $target =~ ^[0-9]+$ ]]; then
    printf '%s\n' "$target"
  elif [[ $target =~ /pull/([0-9]+)(/|$) ]]; then
    printf '%s\n' "${BASH_REMATCH[1]}"
  else
    printf '\n'
  fi
  return 0
}
