#!/usr/bin/env bash
#
# PR 本文が宣言した差分規模と、実際の差分を突き合わせる。
#
# 強制力の序列で層 1（AGENTS.md「強制力の序列」）。層 2（git hooks）には置けない
# ——— push の時点では PR がまだ無く、本文も存在しないため。
#
# 使い方:
#   bash harness/ci/pr-body-diff.sh <本文のファイル> <基点の rev> <先端の rev>
#
# ローカルで確かめるとき:
#   bash harness/ci/pr-body-diff.sh /tmp/body.md origin/main HEAD
#
# 本文に置く宣言は次の 1 行（ちょうど 1 行）。
#
#   差分規模: 3 ファイル / +61 -0
#
# Why（`差分規模` という綴り）: harness/records/TEMPLATE.md が記録に要求している
# 綴りと同じものを使う。新しい語彙を足さずに済む（rules/naming.md
# 「語を増やすときは、既にある語で言えないかを先に確かめる」）。
set -euo pipefail

if [ "$#" -ne 3 ]; then
  echo "使い方: $0 <本文のファイル> <基点の rev> <先端の rev>" >&2
  exit 2
fi

body_file="$1"
base_rev="$2"
head_rev="$3"

if [ ! -f "$body_file" ]; then
  echo "本文のファイルが読めません: $body_file" >&2
  exit 2
fi

# 数の並びだけを拾う。前後の装飾（`**`・表のセル・箇条書き）は見ない。
declaration_pattern='[0-9]+[[:space:]]*ファイル[[:space:]]*/[[:space:]]*\+[0-9]+[[:space:]]*-[0-9]+'

# 宣言の行は「`差分規模` と数の並びの両方を持つ行」。片方だけの行は拾わない
# （本文が `差分規模` を地の文で書くことも、別の PR の数を引くこともあるため）。
declarations="$(grep -E "差分規模" "$body_file" | grep -E "$declaration_pattern" || true)"
declaration_count="$(printf '%s' "$declarations" | grep -c . || true)"

if [ "$declaration_count" -eq 0 ]; then
  cat >&2 <<'EOF'
PR 本文に差分規模の宣言がありません。

次の 1 行を本文に置いてください（数は git から出し直したものを書く）。

    差分規模: <ファイル数> ファイル / +<追加行> -<削除行>

    git diff --numstat origin/main...HEAD

Why: 本文の数がレビュー対応の push で古くなる形が、層 4（AGENTS.md）へ介入した後も
再発している。出し直せる数は層 1 で突き合わせる（AGENTS.md「ツールで落とせるものは
規約の文に留めない」）。
EOF
  exit 1
fi

if [ "$declaration_count" -gt 1 ]; then
  echo "差分規模の宣言が ${declaration_count} 行あります。どれを突き合わせるかが決まらないので、1 行にしてください。" >&2
  printf '%s\n' "$declarations" >&2
  exit 1
fi

numbers="$(printf '%s' "$declarations" | grep -oE "$declaration_pattern")"
declared_files="$(printf '%s' "$numbers" | sed -E 's/^([0-9]+).*/\1/')"
declared_additions="$(printf '%s' "$numbers" | sed -E 's/.*\+([0-9]+)[[:space:]]*-[0-9]+$/\1/')"
declared_deletions="$(printf '%s' "$numbers" | sed -E 's/.*-([0-9]+)$/\1/')"

numstat="$(git diff --numstat "${base_rev}...${head_rev}")"

# バイナリのファイルは numstat が行数を `-` で返す。行としては数えられないので 0 として
# 足し、ファイル数には数える（GitHub の「Files changed」と同じ扱い）。
actual_files="$(printf '%s' "$numstat" | grep -c . || true)"
actual_additions="$(printf '%s\n' "$numstat" | awk -F'\t' '$1 ~ /^[0-9]+$/ { sum += $1 } END { print sum + 0 }')"
actual_deletions="$(printf '%s\n' "$numstat" | awk -F'\t' '$2 ~ /^[0-9]+$/ { sum += $2 } END { print sum + 0 }')"

echo "本文の宣言: ${declared_files} ファイル / +${declared_additions} -${declared_deletions}"
echo "実際の差分: ${actual_files} ファイル / +${actual_additions} -${actual_deletions}  (git diff --numstat ${base_rev}...${head_rev})"

matches_files=$([ "$declared_files" = "$actual_files" ] && echo yes || echo no)
matches_additions=$([ "$declared_additions" = "$actual_additions" ] && echo yes || echo no)
matches_deletions=$([ "$declared_deletions" = "$actual_deletions" ] && echo yes || echo no)

if [ "$matches_files" = yes ] && [ "$matches_additions" = yes ] && [ "$matches_deletions" = yes ]; then
  echo "差分規模の宣言は実際の差分と一致しています。"
  exit 0
fi

cat >&2 <<EOF

PR 本文の差分規模が実際の差分と合っていません。本文を直してください。

    差分規模: ${actual_files} ファイル / +${actual_additions} -${actual_deletions}

本文の数だけを直して終わりにしないこと。同じ push で古くなった記述が本文の他の箇所にも
あります（AGENTS.md「記録・PR 本文に数値や主張を書く前に」）。
EOF
exit 1
