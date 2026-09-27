#!/usr/bin/env bash
#
# Bash のコマンド文字列を、区切った 1 つずつのコマンドへ割る。source して使う。
#
# 使うのは lib/git-push-command.sh と lib/gh-pr-merge-command.sh。**割り方を 1 箇所に置く** —
# 2 箇所に持つと、片方だけ直ったときに同じコマンドの読み方が判定ごとに割れる。

# コマンド文字列を、引用符の中身を落としてから `&&` / `||` / `;` / `|` / `&` / 括弧 / 改行で
# 区切り、1 行に 1 つずつ出す。
#
# 引用符の中身は 1 語の引数 `Q` として残す。消すと `-m "x" push` の `-m` が次の語を食う。
#
# Why not（引用符の中身を残す）: `git commit -m "then git push"` の中身まで区切った先頭に見える。
command_segments_of() {
  printf '%s' "$1" \
    | sed -E "s/'[^']*'/Q/g; s/\"([^\"\\\\]|\\\\.)*\"/Q/g" \
    | tr ';&|()' '\n\n\n\n\n'
  printf '\n'
}
