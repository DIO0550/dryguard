#!/usr/bin/env bash
#
# ディレクトリからのドメイン推定がどこで外れ、判定をどう変えるかを数える（Issue #36）。
#
# 対象の置き方ごとに「正解のドメイン」を domains.tsv に書いておき、scan の結果と
# 突き合わせる。候補ペアごとに、推定のまま出た判定と、ディレクトリを見る 2 つの
# シグナル（モジュール距離・呼び出し元ドメイン）を正解のドメインで置き換えて
# 出し直した判定を並べる。
#
# なぜプロダクトの外にあるか: domains.tsv はドメイン宣言の記法そのものではなく、
# 記法を決めるための材料。先に dryguard.toml へ読み込む形を作ると、この Issue が
# 決めようとしている粒度と優先順位を先取りすることになる。
#
# 使い方:
#   bash harness/domain-probe/probe.sh <置き方のディレクトリ>
#   （例: bash harness/domain-probe/probe.sh tests/layouts/layered）
#
# <置き方のディレクトリ> は src/（走査の根）と domains.tsv を持つ。
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

readonly LAYOUT="${1:?置き方のディレクトリを指定してください}"
readonly SRC="$LAYOUT/src"
readonly TRUTH="$LAYOUT/domains.tsv"

if [ ! -d "$SRC" ] || [ ! -f "$TRUTH" ]; then
  echo "probe: $LAYOUT に src/ と domains.tsv がありません" >&2
  exit 1
fi

# 呼び出し元ドメインは LSP が無いと測れない。無いまま走らせると、2 つのシグナルの
# 片方が黙って「測れない」になり、外れた数が少なく出る（フェイルオープンかつサイレント）
export PATH="$PWD/node_modules/.bin:$PATH"
if ! command -v typescript-language-server >/dev/null 2>&1; then
  echo "probe: typescript-language-server がありません（CI と同じく pnpm install --frozen-lockfile で入れる）" >&2
  exit 1
fi

cargo build --release --quiet
readonly BIN="target/release/dryguard"

WORK="$(mktemp -d)"
readonly WORK
trap 'rm -rf "$WORK"' EXIT

"$BIN" scan "$SRC" --format json --explain > "$WORK/scan.json"

# domains.tsv を「ファイル × 行」の表へ展開する。
#
# 1 列目: src/ からの相対パス。`/` で終われば、その下のファイルすべて
# 2 列目: `*`（ファイル全体）か関数名。関数名は宣言の行へ引き直す
# 3 列目: 正解のドメイン
#
# 関数名で書くのは、行番号で書くとコーパスを直すたびに表がずれるため。
# 引き直せなければ止める（黙って落とすと、その関数が表から消える）。
: > "$WORK/truth.tsv"
while IFS=$'\t' read -r path target domain; do
  [ -z "$path" ] && continue
  if [ "${path%/}" != "$path" ]; then
    find "$SRC/$path" -name '*.ts' | sort | while read -r file; do
      printf '%s\t*\t%s\n' "${file#"$SRC/"}" "$domain"
    done >> "$WORK/truth.tsv"
    continue
  fi
  if [ ! -f "$SRC/$path" ]; then
    echo "probe: domains.tsv が指すファイルがありません: $path" >&2
    exit 1
  fi
  if [ "$target" = "*" ]; then
    printf '%s\t*\t%s\n' "$path" "$domain" >> "$WORK/truth.tsv"
    continue
  fi
  # 見つからないときに pipefail で黙って落ちないよう、件数は下で数えて止める
  lines="$(grep -n -E "function ${target}[<(]" "$SRC/$path" | cut -d: -f1 || true)"
  if [ "$(printf '%s\n' "$lines" | grep -c .)" -ne 1 ]; then
    echo "probe: $path に関数 $target の宣言が 1 つだけ見つかりません" >&2
    exit 1
  fi
  printf '%s\t%s\t%s\n' "$path" "$lines" "$domain" >> "$WORK/truth.tsv"
done < "$TRUTH"

# 突き合わせる。
#
# 判定を出し直す決定木は src/classification.rs の写しで、**写しがずれていないことを
# 先に確かめる**: 推定のままの傾きで出し直した判定が、scan が出した判定とすべての
# ペアで一致しなければ止める。
jq -r \
  --rawfile truth "$WORK/truth.tsv" \
  --arg src "$(cd "$SRC" && pwd)" \
  --arg prefix "$SRC/" '
  def truth_table:
    $truth | split("\n") | map(select(length > 0) | split("\t"))
    | reduce .[] as [$path, $line, $domain] ({};
        .[$path] += (if $line == "*" then {default: $domain}
                     else {lines: ((.[$path].lines // {}) + {($line): $domain})} end));

  def domain_of($table; $loc):
    ($loc.file | ltrimstr($prefix)) as $path
    | ($table[$path] // error("domains.tsv に無いファイル: \($path)")) as $entry
    | ($entry.lines[($loc.line | tostring)] // $entry.default
       // error("domains.tsv に無い関数: \($path):\($loc.line)"));

  # 呼び出し元は scan がディレクトリでしか出さない。そのディレクトリ直下の
  # ファイルが正解で 1 つのドメインにまとまるときだけ引き直せる
  def directory_domains($table; $directory):
    ($directory | ltrimstr($src) | ltrimstr("/")) as $relative
    | [$table | to_entries[]
        | select((.key | split("/") | .[:-1] | join("/")) == $relative)
        | .value | (.default // empty), (.lines // {} | .[])]
    | unique;

  def row_text:
    "\(.a) <-> \(.b)\t\(.verdict) -> \(.declared)\t正解: \(if .same_domain then "同じ" else "別" end)"
    + " / 距離 \(.distance_steps) 段 (\(.distance_lean))"
    + " / 呼び出し元 \(.caller_lean) -> \(.declared_caller_lean) [\(.caller_remapped)]";

  def reason($pair; $signal): $pair.reasons[] | select(.signal == $signal);

  def placement($import; $distance):
    if $import == "toward-extract" then "same"
    elif $import == "neither" then "undecidable"
    elif $distance == "toward-do-not-extract" then "separate"
    else "undecidable" end;

  def domain_match($placement; $caller):
    if $caller == "neither" then $placement
    elif $caller == "toward-extract" then
      (if $placement == "separate" then "undecidable" else "same" end)
    else
      (if $placement == "same" then "undecidable" else "separate" end)
    end;

  def verdict($similar; $import; $distance; $signature; $caller):
    placement($import; $distance) as $placement
    | domain_match($placement; $caller) as $match
    | if ($similar | not) then "REVIEW"
      elif $match == "separate" then "DO-NOT-EXTRACT"
      elif $match == "undecidable" then "REVIEW"
      elif $signature == "toward-do-not-extract" then "REVIEW"
      elif $signature == "toward-extract" then "EXTRACT-CANDIDATE"
      elif $placement == "same" then "EXTRACT-CANDIDATE"
      else "REVIEW" end;

  def jaccard($a; $b):
    ($a | map(select(. as $x | $b | index($x)))) as $shared
    | ($shared | length) / (($a + $b | unique) | length);

  truth_table as $table
  | [.candidate_pairs[] as $pair
     | domain_of($table; $pair.location_a) as $domain_a
     | domain_of($table; $pair.location_b) as $domain_b
     | reason($pair; "structural-similarity") as $structure
     | ($structure.value.similarity >= $structure.threshold) as $similar
     | reason($pair; "import-overlap").lean as $import
     | reason($pair; "module-distance") as $distance
     | reason($pair; "type-signature-match").lean as $signature
     | reason($pair; "caller-domain-overlap") as $caller
     | (if $domain_a == $domain_b then "toward-extract" else "toward-do-not-extract" end)
       as $declared_distance
     | (if $caller.value.status != "measured" then {lean: "neither", remapped: "not-measured"}
        else
          ([$caller.value.callers_a[].domain | directory_domains($table; .)]) as $dirs_a
          | ([$caller.value.callers_b[].domain | directory_domains($table; .)]) as $dirs_b
          | if ($dirs_a + $dirs_b | any(length != 1)) then {lean: "neither", remapped: "mixed-directory"}
            else
              jaccard($dirs_a | map(.[0]) | unique; $dirs_b | map(.[0]) | unique) as $overlap
              | {lean: (if $overlap >= $caller.threshold then "toward-extract"
                        else "toward-do-not-extract" end),
                 remapped: "remapped"}
            end
        end) as $declared_caller
     | {
         a: "\($pair.location_a.file | ltrimstr($prefix)):\($pair.location_a.line)",
         b: "\($pair.location_b.file | ltrimstr($prefix)):\($pair.location_b.line)",
         same_domain: ($domain_a == $domain_b),
         distance_steps: $distance.value.steps,
         distance_lean: $distance.lean,
         distance_misfires: ($distance.lean != $declared_distance),
         caller_lean: $caller.lean,
         declared_caller_lean: $declared_caller.lean,
         caller_remapped: $declared_caller.remapped,
         caller_misfires: ($caller.lean != "neither" and $declared_caller.remapped == "remapped"
                           and $caller.lean != $declared_caller.lean),
         verdict: $pair.verdict,
         replayed: verdict($similar; $import; $distance.lean; $signature; $caller.lean),
         declared: verdict($similar; $import; $declared_distance; $signature; $declared_caller.lean)
       }] as $rows
  | if ($rows | any(.verdict != .replayed)) then
      error("決定木の写しが src/classification.rs とずれている: " +
            ($rows | map(select(.verdict != .replayed)) | first | tostring))
    else . end
  | ($rows | map(select(.verdict != .declared))) as $changed
  | "候補ペア\t\($rows | length)",
    "正解で同じドメイン\t\($rows | map(select(.same_domain)) | length)",
    "モジュール距離が外れた\t\($rows | map(select(.distance_misfires)) | length)",
    "呼び出し元ドメインが外れた\t\($rows | map(select(.caller_misfires)) | length)",
    "呼び出し元を引き直せなかった（ディレクトリに別ドメインが同居）\t\($rows | map(select(.caller_remapped == "mixed-directory")) | length)",
    "正解のドメインで出し直すとラベルが変わる\t\($changed | length)",
    "",
    "ラベルの移り変わり（推定 -> 正解）\t件数",
    ($changed | group_by("\(.verdict) -> \(.declared)")
     | map("\(.[0].verdict) -> \(.[0].declared)\t\(length)") | .[]),
    "",
    "ラベルが変わったペア",
    ($changed[] | row_text),
    "",
    "外れたがラベルは変わらなかったペア",
    ($rows | map(select((.distance_misfires or .caller_misfires) and .verdict == .declared))
     | .[] | row_text)
  ' "$WORK/scan.json"
