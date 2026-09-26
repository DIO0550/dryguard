#!/usr/bin/env bash
#
# Bash のコマンド文字列が `git push` を含むかを判定する。source して使う。
#
# 読み損ねた形は「push ではない」側へ倒す。取りこぼしても層 2（harness/githooks/pre-push）が
# push の時点で同じ検査を走らせるが、push でないコマンドを push と読むと、検査の失敗で
# そのコマンドまで止めてしまう（rules/coding.md「列挙で判定を組むときは、漏れの倒れる向きを選ぶ」）。

# コマンド文字列が `git push` を含むなら 0、含まなければ 1 を返す。
#
# 引用符の中身を落としてから `&&` / `||` / `;` / `|` / `&` / 括弧 / 改行で区切り、
# 区切った 1 つずつの先頭を見る。
#
# Why not（部分文字列 `git push` を探す）: `git commit -m "then git push"` や
# `echo git push` まで push と読み、push しないコマンドを止める。
is_git_push_command() {
  local command="$1"
  local unquoted
  # 引用符の中身は 1 語の引数として残す。消すと `-m "x" push` の `-m` が次の語を食う。
  unquoted="$(printf '%s' "$command" | sed -E "s/'[^']*'/Q/g; s/\"([^\"\\\\]|\\\\.)*\"/Q/g")"

  local segment
  while IFS= read -r segment; do
    if segment_is_git_push "$segment"; then
      return 0
    fi
  done < <(printf '%s\n' "$unquoted" | tr ';&|()' '\n\n\n\n\n')

  return 1
}

# 区切った 1 つ分のコマンドが `git push` なら 0 を返す。
#
# 先頭の環境変数の代入と、`git` の大域オプション（`-C <dir>` / `-c <k=v>` / その他の `-*`）を
# 飛ばした次の語が `push` かを見る。
segment_is_git_push() {
  local words
  read -ra words <<< "$1"

  local i=0
  local count="${#words[@]}"
  while [ "$i" -lt "$count" ] && [[ "${words[$i]}" =~ ^[A-Za-z_][A-Za-z0-9_]*= ]]; do
    i=$((i + 1))
  done

  if [ "$i" -ge "$count" ] || [ "${words[$i]}" != "git" ]; then
    return 1
  fi
  i=$((i + 1))

  while [ "$i" -lt "$count" ]; do
    case "${words[$i]}" in
      -C | -c) i=$((i + 2)) ;;
      -*) i=$((i + 1)) ;;
      *) break ;;
    esac
  done

  [ "$i" -lt "$count" ] && [ "${words[$i]}" = "push" ]
}
