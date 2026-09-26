#!/usr/bin/env bash
#
# PostToolUse（Edit / Write / MultiEdit）で `.rs` を編集したら、そのクレートに
# `cargo fmt` をかけ、`cargo clippy` の診断を Claude へ返す。
#
# ここはゲートではない（AGENTS.md「強制力の序列」の層 3）。発火しない環境では
# 即時性だけが失われ、同じ検査は push 前（harness/githooks/pre-push）と CI が拾う。
#
# 終了コード: 0 = 何もしなかった / 通った。2 = 診断を stderr に出した
# （PostToolUse の exit 2 は stderr を Claude に渡す。編集は取り消されない）。
set -uo pipefail

# jq と cargo が無い環境では黙って通す。検査できないことを理由に編集を止めても、
# 検査の質は上がらない（harness/githooks/pre-push と同じ扱い）。
if ! command -v jq >/dev/null 2>&1 || ! command -v cargo >/dev/null 2>&1; then
  exit 0
fi

file_path="$(jq -r '.tool_input.file_path // empty' 2>/dev/null)"

if [ -z "$file_path" ] || [ "${file_path##*.}" != "rs" ] || [ ! -f "$file_path" ]; then
  exit 0
fi

# 編集したファイルから上へ、最も近い Cargo.toml を探す。
#
# Why not（プロジェクトの根の Cargo.toml に固定する）: harness/phase0/chunk-unit-probe/ は
# 別のクレートで、根で走らせるとそのファイルを fmt も clippy も見ない。
crate_dir="$(cd "$(dirname "$file_path")" && pwd)"
while [ ! -f "${crate_dir}/Cargo.toml" ]; do
  if [ "$crate_dir" = "/" ]; then
    exit 0
  fi
  crate_dir="$(dirname "$crate_dir")"
done
manifest="${crate_dir}/Cargo.toml"

# Why not（`rustfmt --edition 2024 <file>`）: 版を Cargo.toml と二重に持つことになる。
# クレート全体にかけても、CI が fmt を無条件に通しているので編集していないファイルは変わらない。
#
# 整形できないのは構文が壊れているときで、clippy は同じエラーを重ねて出すだけなので走らせない。
if ! fmt_output="$(cargo fmt --manifest-path "$manifest" 2>&1)"; then
  {
    echo "post-edit-rust: cargo fmt が失敗しました（${file_path}）"
    printf '%s\n' "$fmt_output"
  } >&2
  exit 2
fi

if ! clippy_output="$(cargo clippy --quiet --manifest-path "$manifest" --all-targets -- -D warnings 2>&1)"; then
  {
    echo "post-edit-rust: cargo clippy が警告を出しました（${file_path}）"
    printf '%s\n' "$clippy_output"
  } >&2
  exit 2
fi

exit 0
