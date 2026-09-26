#!/usr/bin/env bash
#
# .claude/hooks/pre-push-check.sh のテスト。
#
# 前半はコマンドの判定（lib/git-push-command.sh）を、後半はフックを stdin の JSON から
# end-to-end で走らせる。後半は使い捨ての git リポジトリに**落ちる / 通る検査**を置き、
# どちらが呼ばれたかと終了コードを突き合わせる（本物の cargo を毎回走らせないため）。
#
# 使い方: bash .claude/hooks/pre-push-check-test.sh
set -euo pipefail

hooks_dir="$(cd "$(dirname "$0")" && pwd)"
hook="${hooks_dir}/pre-push-check.sh"
# shellcheck source=lib/git-push-command.sh
source "${hooks_dir}/lib/git-push-command.sh"

workspace="$(mktemp -d)"
trap 'rm -rf "$workspace"' EXIT

failures=0

report() {
  local name="$1"
  local reason="$2"
  local output="${3:-}"

  if [ -z "$reason" ]; then
    echo "ok   ${name}"
    return 0
  fi

  echo "FAIL ${name}: ${reason}"
  if [ -n "$output" ]; then
    printf '%s\n' "$output" | sed 's/^/     | /'
  fi
  failures=$((failures + 1))
}

expect_push() {
  local name="$1"
  local command="$2"
  local reason=""
  if ! is_git_push_command "$command"; then
    reason="push として読まなかった: ${command}"
  fi
  report "$name" "$reason"
}

expect_not_push() {
  local name="$1"
  local command="$2"
  local reason=""
  if is_git_push_command "$command"; then
    reason="push として読んだ: ${command}"
  fi
  report "$name" "$reason"
}

expect_push "素の git push に当たる" 'git push -u origin feature'
expect_push "引数の無い git push に当たる" 'git push'
# 移植元（design-composer）が実際に取りこぼしていた形。
expect_push "&& の後ろの git push に当たる" 'cargo fmt && git push origin feature'
expect_push "; の後ろの git push に当たる" 'git commit -m done; git push'
expect_push "|| の後ろの git push に当たる" 'git push || git push --force-with-lease'
expect_push "改行の後ろの git push に当たる" 'git add src/lib.rs
git push'
expect_push "パイプの前の git push に当たる" 'git push 2>&1 | tail -5'
expect_push "サブシェルの中の git push に当たる" '(cd sub && git push)'
expect_push "環境変数の代入の後ろの git push に当たる" 'GIT_TRACE=1 git push'
expect_push "-C の後ろの git push に当たる" 'git -C /home/user/dryguard push'
expect_push "-c の後ろの git push に当たる" 'git -c push.default=current push'
expect_push "-- 付きの大域オプションの後ろの git push に当たる" 'git --no-pager push'

expect_not_push "push でない git のコマンドには当たらない" 'git status'
expect_not_push "push で始まる別のサブコマンドには当たらない" 'git pushx'
# 引用符の中身まで読むと、push しないコマンドを検査の失敗で止めてしまう。
expect_not_push "コミットメッセージの中の git push には当たらない" 'git commit -m "then git push"'
expect_not_push "単引用符の中の ; git push には当たらない" "echo 'a; git push origin'"
expect_not_push "echo の引数の git push には当たらない" 'echo git push'
expect_not_push "push を引数に取る別のコマンドには当たらない" 'git log --grep push'

# --- フックを end-to-end で走らせる ---

# 呼ばれたら印を残す検査を置いたリポジトリを作る。
# 印を見るのは、終了コードだけだと「呼ばずに通した」と「呼んで通った」が区別できないため。
make_repo() {
  local dir="$1"
  local check_exit="$2"
  mkdir -p "${dir}/harness/githooks"
  git -C "$dir" init -q
  cat > "${dir}/harness/githooks/pre-push" <<EOF
#!/usr/bin/env bash
echo called > "${dir}/called"
echo "pre-push: fake check output"
exit ${check_exit}
EOF
  chmod +x "${dir}/harness/githooks/pre-push"
}

run_hook() {
  local dir="$1"
  local input="$2"
  local status=0
  # stderr だけを受ける。PreToolUse が exit 2 のときに Claude へ返すのは stderr だけなので、
  # 検査の出力が stdout に出ていたら返らない。
  hook_output="$(printf '%s' "$input" | CLAUDE_PROJECT_DIR="$dir" bash "$hook" 2>&1 >/dev/null)" || status=$?
  hook_status="$status"
}

bash_input() {
  jq -n --arg c "$1" '{tool_name: "Bash", tool_input: {command: $c}}'
}

expect_hook() {
  local name="$1"
  local check_exit="$2"
  local input="$3"
  local expected_status="$4"
  local expected_called="$5"
  local expected_message="$6"
  local repo_kind="${7:-git}"
  local dir
  dir="$(mktemp -d "${workspace}/repo.XXXXXX")"
  make_repo "$dir" "$check_exit"
  if [ "$repo_kind" = "not-git" ]; then
    rm -rf "${dir}/.git"
  fi

  run_hook "$dir" "$input"

  local called=no
  if [ -e "${dir}/called" ]; then
    called=yes
  fi

  local reason=""
  if [ "$hook_status" -ne "$expected_status" ]; then
    reason="終了コードが ${expected_status} ではなく ${hook_status}"
  elif [ "$called" != "$expected_called" ]; then
    reason="検査を呼んだか: ${expected_called} を期待したが ${called}"
  elif [ -n "$expected_message" ] && ! printf '%s' "$hook_output" | grep -qF "$expected_message"; then
    reason="出力に「${expected_message}」が無い"
  fi
  report "$name" "$reason" "$hook_output"
}

expect_hook "検査が落ちたら push を止めて出力を返す" 1 "$(bash_input 'cargo fmt && git push')" 2 yes "pre-push: fake check output"
expect_hook "検査が通れば push を通す" 0 "$(bash_input 'git push')" 0 yes ""
expect_hook "push でなければ検査を呼ばない" 1 "$(bash_input 'git status')" 0 no ""
expect_hook "入力の JSON が読めなければ黙って通す" 1 'not json' 0 no ""
expect_hook "command を持たない入力は黙って通す" 1 '{"tool_name": "Bash", "tool_input": {}}' 0 no ""
expect_hook "git の作業ツリーでなければ黙って通す" 1 "$(bash_input 'git push')" 0 no "" not-git

if [ "$failures" -ne 0 ]; then
  echo "${failures} 件失敗しました。"
  exit 1
fi

echo "すべて通りました。"
