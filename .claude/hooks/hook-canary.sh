#!/usr/bin/env bash
#
# PreToolUse（Bash）で `echo hook-canary` を必ず止める（層 3 のカナリア）。
#
# push の前に `echo hook-canary` を 1 度実行すると、止められればこのセッションでフックが
# 発火しており、通ってしまえば発火していない。フェイルオープンかつサイレントな層 3 の不発を
# 見える形にするためだけのもので、**止めているのは害の無いコマンド 1 つだけ**。検出が
# 失敗してもガードは破れない（ゲートは層 2 と層 1 にある）。
#
# **判定も出力も bash の組み込みだけで行う。** PreToolUse は exit 2 以外の異常終了を
# 素通りさせるので、`jq` が無くて exit 127 で落ちると、配線が読まれていないのと同じ
# 見え方になる。それではフックが動いていたのに不発と報告することになる。
#
# 終了コード: `echo hook-canary` なら 2（stderr を Claude に返す）。それ以外は 0。
set -u

# Why not（`jq` が在れば `jq` で読む）: 環境によって判定の経路が変わり、同じ入力で
# 答えが割れうる。値が `echo hook-canary` そのもののときだけ当てるので正規表現 1 本で足りる。
canary_pattern='"command"[[:space:]]*:[[:space:]]*"echo[[:space:]]+hook-canary[[:space:]]*"'

input=''
IFS= read -r -d '' input || true

if ! [[ $input =~ $canary_pattern ]]; then
  exit 0
fi

# カナリアが止められても、他のフックはこれらが欠けていれば同じフェイルオープンで
# 黙って素通りする。発火していることと、中身が検査できていることは別なので併せて出す。
# 一覧は他のフックが実際に呼ぶもの（jq: pre-push-check.sh / post-edit-rust.sh、
# cargo: post-edit-rust.sh と、pre-push-check.sh が呼ぶ harness/githooks/pre-push）。
#
# Why not（design-composer と同じく python3 を見る）: このリポジトリのフックは python3 を
# 呼ばない。欠けていても素通りするフックが無いのに名前を出すと、直す必要の無いものを直させる。
missing=''
for tool in jq cargo; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    missing="${missing} ${tool}"
  fi
done

printf '%s\n' "hook-canary: このセッションでは PreToolUse のフックが発火しています" >&2
if [ -n "$missing" ]; then
  printf '%s\n' "hook-canary: ただし次のコマンドが無く、それを使うフックは黙って素通りします:${missing}" >&2
fi

exit 2
