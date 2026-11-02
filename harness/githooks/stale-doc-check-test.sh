#!/usr/bin/env bash
# stale-doc-check のテスト。一時リポジトリで、差分に載らない名指しのコメントだけが出ることを見る。
set -uo pipefail

script="$(cd "$(dirname "$0")" && pwd)/stale-doc-check"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cd "$work"
git init -q . && git config user.email t@t && git config user.name t

mkdir src
cat > src/a.rs <<'RS'
/// `limit_of` が返す上限。
pub const LIMIT_OF_ROWS: usize = 3;
// LIMIT_OF_ROWS を読む。
fn use_it() {}
// 無関係なコメント。
RS
git add -A && git commit -qm base

sed -i 's/= 3;/= 5;/; s/pub const LIMIT_OF_ROWS: usize/pub const LIMIT_OF_ROWS: u32/' src/a.rs
git commit -qam change

fail=0
out="$(bash "$script" HEAD~1)"
if ! grep -q 'LIMIT_OF_ROWS を読む' <<< "$out"; then echo "FAIL: 名指しのコメントが出ない"; fail=1; fi
if grep -q '無関係' <<< "$out"; then echo "FAIL: 無関係なコメントが出た"; fail=1; fi

git commit -q --allow-empty -m noop
out="$(bash "$script" HEAD)"
if [ -n "$out" ]; then echo "FAIL: 差分が無いのに出力した: $out"; fail=1; fi

[ "$fail" -eq 0 ] && echo "stale-doc-check-test: ok"
exit "$fail"
