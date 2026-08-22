#!/usr/bin/env bash
#
# Phase 0 の仮説検証。実リポジトリの TS に compare を当てて、ラベルの出方を数える。
#
# なぜプロダクトの外にあるか: compare は 2 箇所しか受けないので、頻度を数えるには
# ペアの列挙が要る。列挙そのものは scan の仕事で Phase 1 に置かれている
# (docs/dryguard-plan.md「Phase 1: Stage 1 を厚くする」)。検証のために先回りで
# 生やすと、src/cli.rs が避けている「受け取るだけで結果が出ないサブコマンド」に
# なる。ここは検証の足場であって、プロダクトの機能ではない。
#
# 使い方:
#   bash harness/phase0/verify.sh <対象ディレクトリ> [出力先ディレクトリ]
#
# 環境変数:
#   DRYGUARD_MAX_PAIRS  ペア数の上限（既定 200000）。超えたら間引かずに止める
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

readonly TARGET="${1:?対象ディレクトリを指定してください}"
readonly OUT_DIR="${2:-harness/phase0/out}"
readonly MAX_PAIRS="${DRYGUARD_MAX_PAIRS:-200000}"

if [ ! -d "$TARGET" ]; then
  echo "verify: 対象ディレクトリがありません: $TARGET" >&2
  exit 1
fi

mkdir -p "$OUT_DIR"
readonly SEEDS="$OUT_DIR/seeds.txt"
readonly VERDICTS="$OUT_DIR/verdicts.txt"
readonly DO_NOT_EXTRACT="$OUT_DIR/do-not-extract.txt"
readonly UNCHUNKABLE="$OUT_DIR/unchunkable.txt"

echo "verify: ビルド"
cargo build --release
readonly BIN="target/release/dryguard"

# 関数の始まりに見える行を集める。compare は「その行を含む関数」を切り出すので、
# 関数 1 つにつき 1 行あればよい。
#
# 判定条件は src/syntax/chunk.rs の is_function_header に合わせてある。**合わせて
# あるだけで、同じ実装ではない。** ずれた場合はチャンクを切り出せない位置が種に
# 混ざるが、それは compare が NoEnclosingFunction で落として unchunkable.txt に
# 出るので、黙って取りこぼす形にはならない（数が合わなければそこを見る）。
echo "verify: 関数の位置を集める"
find "$TARGET" \
  \( -name node_modules -o -name .git -o -name dist -o -name build \) -prune -o \
  \( -name '*.ts' -o -name '*.tsx' \) -print |
  sort |
  while read -r file; do
    awk -v path="$file" '
      {
        line = $0
        sub(/^[ \t]+/, "", line)
        sub(/[ \t]+$/, "", line)

        if (line ~ /^}/) next
        if (line !~ /\{$/) next

        # close は awk の組み込み関数名なので変数に使えない
        open_paren = index(line, "(")
        if (open_paren == 0) next
        close_paren = 0
        for (i = length(line); i > 0; i--) {
          if (substr(line, i, 1) == ")") { close_paren = i; break }
        }
        if (close_paren <= open_paren) next

        word = line
        sub(/[^A-Za-z0-9_$].*$/, "", word)
        if (word == "if" || word == "for" || word == "while" || word == "switch" \
            || word == "catch" || word == "do" || word == "else") next

        print path ":" NR
      }
    ' "$file"
  done > "$SEEDS"

readonly SEED_COUNT=$(wc -l < "$SEEDS" | tr -d ' ')
readonly PAIR_COUNT=$(( SEED_COUNT * (SEED_COUNT - 1) / 2 ))

echo "verify: 関数 $SEED_COUNT 件 / ペア $PAIR_COUNT 件"

if [ "$SEED_COUNT" -lt 2 ]; then
  echo "verify: 比較できる関数が 2 件未満です" >&2
  exit 1
fi

# 上限を超えたら間引かずに止める。黙って一部だけ回すと、出た件数が全体の
# 何割なのかが読めない（「網羅した」と読み違える）。
if [ "$PAIR_COUNT" -gt "$MAX_PAIRS" ]; then
  echo "verify: ペア数が上限 $MAX_PAIRS を超えています。対象を絞るか DRYGUARD_MAX_PAIRS を上げてください" >&2
  exit 1
fi

echo "verify: 比較"
: > "$VERDICTS"
: > "$DO_NOT_EXTRACT"
: > "$UNCHUNKABLE"

mapfile -t seeds < "$SEEDS"
done_pairs=0

for (( i = 0; i < SEED_COUNT - 1; i++ )); do
  for (( j = i + 1; j < SEED_COUNT; j++ )); do
    if output=$("$BIN" compare "${seeds[i]}" "${seeds[j]}" --explain 2>"$OUT_DIR/.stderr"); then
      label=$(printf '%s\n' "$output" | head -1 | sed -n 's/^\[\([A-Z-]*\)\].*/\1/p')
      printf '%s\t%s\t%s\n' "$label" "${seeds[i]}" "${seeds[j]}" >> "$VERDICTS"

      if [ "$label" = "DO-NOT-EXTRACT" ]; then
        printf '%s\n\n' "$output" >> "$DO_NOT_EXTRACT"
      fi
    else
      printf '%s\t%s\t%s\n' "${seeds[i]}" "${seeds[j]}" "$(tr '\n' ' ' < "$OUT_DIR/.stderr")" >> "$UNCHUNKABLE"
    fi

    done_pairs=$(( done_pairs + 1 ))
    if [ $(( done_pairs % 2000 )) -eq 0 ]; then
      echo "verify: $done_pairs / $PAIR_COUNT"
    fi
  done
done

rm -f "$OUT_DIR/.stderr"

echo
echo "=== ラベルごとの件数 ==="
cut -f1 "$VERDICTS" | sort | uniq -c | sort -rn
echo
echo "比較できたペア: $(wc -l < "$VERDICTS" | tr -d ' ') / $PAIR_COUNT"
echo "切り出せなかったペア: $(wc -l < "$UNCHUNKABLE" | tr -d ' ')"
echo
echo "DO-NOT-EXTRACT の一覧: $DO_NOT_EXTRACT"
