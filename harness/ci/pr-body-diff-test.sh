#!/usr/bin/env bash
#
# harness/ci/pr-body-diff.sh のテスト。
#
# 使い捨ての git リポジトリを実際に作って end-to-end で走らせる
# （rules/testing.md「モックは使わない」「依存は実物を使い、結果の状態・戻り値を検証する」）。
#
# 使い方: bash harness/ci/pr-body-diff-test.sh
set -euo pipefail

script="$(cd "$(dirname "$0")" && pwd)/pr-body-diff.sh"
workspace="$(mktemp -d)"
trap 'rm -rf "$workspace"' EXIT

failures=0

# 期待する終了コードと、出力に出るべき文言を突き合わせる。
# 落ちたテストの出力をそのまま出す（どの検査で落ちたのかを読み違えないため）。
#
# Why（終了コードだけを見ない）: 落ちる理由が違っても終了コードは 1 で揃うので、
# 「宣言が 2 行ある」を見なくなった実装も、数の突き合わせを飛ばした実装も
# 素通りする。実際に片方ずつ壊して確かめた（rules/testing.md
# 「assert は『落ちうるか』で見る」）。
expect_result() {
  local name="$1"
  local expected="$2"
  local expected_message="$3"
  local body="$4"
  local body_file="${workspace}/body.md"

  printf '%s' "$body" > "$body_file"

  local actual=0
  local output
  output="$( (cd "$repo" && bash "$script" "$body_file" main feature) 2>&1 )" || actual=$?

  local reason=""
  if [ "$actual" -ne "$expected" ]; then
    reason="終了コードが ${expected} ではなく ${actual}"
  elif ! printf '%s' "$output" | grep -qF "$expected_message"; then
    reason="出力に「${expected_message}」が無い"
  fi

  if [ -z "$reason" ]; then
    echo "ok   ${name}"
    return 0
  fi

  echo "FAIL ${name}: ${reason}"
  printf '%s\n' "$output" | sed 's/^/     | /'
  failures=$((failures + 1))
}

# 基点に 1 ファイル、枝で「1 ファイル追加 / 1 ファイルを +2 -1」を作る。
# 追加・削除・ファイル数のどれか 1 つだけを取り違えても落ちる差分にしてある
# （rules/testing.md「assert は『落ちうるか』で見る」）。
repo="${workspace}/repo"
mkdir -p "$repo"
git -C "$repo" init --quiet --initial-branch=main
git -C "$repo" config user.email "test@example.com"
git -C "$repo" config user.name "test"
printf 'a\nb\nc\n' > "${repo}/kept.txt"
git -C "$repo" add kept.txt
git -C "$repo" commit --quiet -m "base"
git -C "$repo" checkout --quiet -b feature
printf 'a\nB1\nB2\nc\n' > "${repo}/kept.txt"
printf 'new\n' > "${repo}/added.txt"
git -C "$repo" add kept.txt added.txt
git -C "$repo" commit --quiet -m "change"

# 実際の差分は 2 ファイル / +3 -1（kept.txt が +2 -1、added.txt が +1 -0）。
expect_result "宣言が実際と一致していれば通る" 0 "差分規模の宣言は実際の差分と一致しています" '## 差分

差分規模: 2 ファイル / +3 -1
'
expect_result "追加行だけずれていれば落ちる" 1 "差分規模が実際の差分と合っていません" '差分規模: 2 ファイル / +4 -1'
expect_result "削除行だけずれていれば落ちる" 1 "差分規模が実際の差分と合っていません" '差分規模: 2 ファイル / +3 -0'
expect_result "ファイル数だけずれていれば落ちる" 1 "差分規模が実際の差分と合っていません" '差分規模: 1 ファイル / +3 -1'
expect_result "宣言が無ければ落ちる" 1 "差分規模の宣言がありません" '## 差分

`src/lib.rs` を直した。
'
expect_result "箇条書きと太字が付いていても読む" 0 "差分規模の宣言は実際の差分と一致しています" '- **差分規模**: 2 ファイル / +3 -1'
# 同じ宣言を 2 度書けること自体が要件。本文が書式を説明する回に、自分の例で
# 落ちないようにしてある（この検査を入れた PR #209 が実際にそうなった）。
expect_result "同じ宣言が 2 行あっても通る" 0 "差分規模の宣言は実際の差分と一致しています" '差分規模: 2 ファイル / +3 -1

書式はこう書く。

差分規模: 2 ファイル / +3 -1
'
expect_result "2 行のうち片方がずれていれば落ちる" 1 "数が揃っていません" '差分規模: 2 ファイル / +3 -1

差分規模: 1 ファイル / +3 -1
'
# 行の頭に限る理由がここ。表のセルに書いた例まで拾うと、書式を説明した本文が落ちる。
expect_result "表のセルの中の例は宣言にしない" 1 "差分規模の宣言がありません" '| カテゴリ | 例 |
|---|---|
| 正常系 | `差分規模: 2 ファイル / +3 -1` と書く |
'
# 数の並びを持たない雛形は宣言ではない。拾うと、説明だけの本文が
# 「宣言はある」と読まれて突き合わせを飛ばす。
expect_result "書式の雛形は宣言にしない" 1 "差分規模の宣言がありません" '差分規模: <ファイル数> ファイル / +<追加行> -<削除行>'

if [ "$failures" -ne 0 ]; then
  echo "${failures} 件失敗しました。"
  exit 1
fi

echo "すべて通りました。"
