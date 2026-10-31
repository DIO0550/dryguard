#!/usr/bin/env bash
# probe-check のテスト。混ざった形と、通す形（対照）を 1 件ずつ置く。
set -u
here="$(cd "$(dirname "$0")" && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
fail=0

# <期待する終了コード> <名前> <ファイル名> <中身>
check() {
  rm -rf "$tmp/r" && mkdir -p "$tmp/r" && cd "$tmp/r" || exit 1
  git init -q . && git config user.email t@t && git config user.name t
  mkdir -p "$(dirname "$3")"
  printf '%s\n' "$4" > "$3"
  git add -A
  bash "$here/probe-check" >/dev/null 2>&1
  code=$?
  if [ "$code" -ne "$1" ]; then echo "FAIL: $2 (exit $code, 期待 $1)"; fail=1; else echo "ok: $2"; fi
}

check 1 "mod zz_probe を拒否する" src/a.rs $'fn a() {}\n\nmod zz_probe {\n    fn x() { println!("t"); }\n}'
check 1 "mod scratch_dump を拒否する" src/a.rs $'#[cfg(test)]\nmod scratch_dump {}'
check 1 "fixtures の Cargo.lock を拒否する" tests/fixtures/f/Cargo.lock '# lock'
check 0 "通常の tests モジュールは通す" src/a.rs $'#[cfg(test)]\nmod tests {}'
check 0 "fixtures の外の println! は通す" src/a.rs $'fn a() { println!("t"); }'
exit "$fail"
