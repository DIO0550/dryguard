#!/usr/bin/env bash
#
# .claude/hooks/post-edit-rust.sh のテスト。
#
# 使い捨てのクレートを実際に作って end-to-end で走らせる
# （rules/testing.md「モックは使わない」「依存は実物を使い、結果の状態・戻り値を検証する」）。
#
# 使い方: bash .claude/hooks/post-edit-rust-test.sh
set -euo pipefail

hook="$(cd "$(dirname "$0")" && pwd)/post-edit-rust.sh"
workspace="$(mktemp -d)"
trap 'rm -rf "$workspace"' EXIT

failures=0

# PostToolUse が stdin に渡す JSON のうち、フックが読む部分だけを組み立てる。
payload_of() {
  jq -n --arg path "$1" '{tool_name: "Edit", tool_input: {file_path: $path}}'
}

# 期待する終了コードと、出力に出るべき文言・出てはいけない文言を突き合わせる。
# 落ちたテストの出力をそのまま出す（どの検査で落ちたのかを読み違えないため）。
#
# Why（終了コードだけを見ない）: fmt で落ちても clippy で落ちても終了コードは 2 で揃うので、
# どちらの段で落ちたのかを見ないと「fmt が落ちたら clippy を走らせない」を確かめられない。
expect_result() {
  local name="$1"
  local expected="$2"
  local expected_message="$3"
  local unexpected_message="$4"
  local payload="$5"

  local actual=0
  local output
  output="$(printf '%s' "$payload" | CLAUDE_PROJECT_DIR="$workspace" bash "$hook" 2>&1)" || actual=$?

  local reason=""
  if [ "$actual" -ne "$expected" ]; then
    reason="終了コードが ${expected} ではなく ${actual}"
  elif [ -n "$expected_message" ] && ! printf '%s' "$output" | grep -qF "$expected_message"; then
    reason="出力に「${expected_message}」が無い"
  elif [ -n "$unexpected_message" ] && printf '%s' "$output" | grep -qF "$unexpected_message"; then
    reason="出力に「${unexpected_message}」が出た"
  fi

  if [ -z "$reason" ]; then
    echo "ok   ${name}"
    return 0
  fi

  echo "FAIL ${name}: ${reason}"
  printf '%s\n' "$output" | sed 's/^/     | /'
  failures=$((failures + 1))
}

# 依存を持たないクレートを作る。`[workspace]` を置くのは、外側のクレートの中に
# 内側のクレートを置いたときに cargo が外側をワークスペースの根と取り違えないため。
new_crate() {
  local dir="$1"
  mkdir -p "${dir}/src"
  cat > "${dir}/Cargo.toml" <<'EOF'
[package]
name = "probe"
version = "0.1.0"
edition = "2024"

[workspace]
EOF
}

# clippy の警告（`needless_return`）を 1 件持つ、整形済みのソース。
# 警告の名前で突き合わせるので、dead_code など別の警告で落ちた実装は通らない。
warned_source='pub fn one() -> i32 {
    return 1;
}
'

clean_source='pub fn one() -> i32 {
    1
}
'

# --- クレートの外側と、その中に置いた別のクレート ---
outer="${workspace}/outer"
inner="${outer}/nested/inner"
new_crate "$outer"
new_crate "$inner"
printf '%s' "$clean_source" > "${outer}/src/lib.rs"
printf '%s' "$warned_source" > "${inner}/src/lib.rs"

# 外側のクレートに警告を 1 件置いておく。.rs 以外を編集して 0 で返るのが、
# 「何も走らせなかった」からであって「走らせて通った」からではないことを確かめるため
# （rules/testing.md「『出ない』を確かめるときは、対照を 1 件置く」）。
warned="${workspace}/warned"
new_crate "$warned"
printf '%s' "$warned_source" > "${warned}/src/lib.rs"
printf 'note\n' > "${warned}/README.md"

expect_result ".rs 以外のファイルを編集したら何も走らせない" \
  0 "" "needless_return" "$(payload_of "${warned}/README.md")"

expect_result "clippy の警告があれば診断を返して 2 で終わる" \
  2 "needless_return" "" "$(payload_of "${warned}/src/lib.rs")"

expect_result "編集したファイルから最も近い Cargo.toml のクレートを検査する（内側）" \
  2 "needless_return" "" "$(payload_of "${inner}/src/lib.rs")"

expect_result "編集したファイルから最も近い Cargo.toml のクレートを検査する（外側）" \
  0 "" "needless_return" "$(payload_of "${outer}/src/lib.rs")"

expect_result "file_path を持たない入力では何も走らせない" \
  0 "" "" '{"tool_name": "Edit", "tool_input": {}}'

expect_result "存在しないファイルでは何も走らせない" \
  0 "" "" "$(payload_of "${warned}/src/missing.rs")"

# --- 整形 ---
unformatted="${workspace}/unformatted"
new_crate "$unformatted"
printf 'pub fn one()->i32{1}\n' > "${unformatted}/src/lib.rs"

expect_result "整形されていないファイルは整形して 0 で終わる" \
  0 "" "" "$(payload_of "${unformatted}/src/lib.rs")"

if diff -q <(printf '%s' "$clean_source") "${unformatted}/src/lib.rs" >/dev/null; then
  echo "ok   整形した結果がファイルに書き戻されている"
else
  echo "FAIL 整形した結果がファイルに書き戻されている"
  sed 's/^/     | /' "${unformatted}/src/lib.rs"
  failures=$((failures + 1))
fi

# --- 構文エラー ---
broken="${workspace}/broken"
new_crate "$broken"
printf 'pub fn one() -> i32 {\n' > "${broken}/src/lib.rs"

expect_result "整形できなければ fmt の診断を返し、clippy は走らせない" \
  2 "post-edit-rust: cargo fmt" "post-edit-rust: cargo clippy" "$(payload_of "${broken}/src/lib.rs")"

if [ "$failures" -ne 0 ]; then
  echo "${failures} 件のテストが落ちました"
  exit 1
fi
echo "すべて通りました"
