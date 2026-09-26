#!/usr/bin/env bash
#
# ディレクトリからのドメイン推定がどこで外れ、判定をどう変えるかを数える（Issue #36）。
#
# 対象の置き方ごとに「正解のドメイン」を domains.tsv に書いておき、scan の結果と
# 突き合わせる。候補ペアごとに、推定のまま出た判定と、ディレクトリを見る 3 つの
# シグナル（モジュール距離・呼び出し元ドメイン・呼び出し先ドメイン）を正解の
# ドメインで置き換えて出し直した判定を並べる。
#
# 置き方のディレクトリに dryguard.toml（ドメインの宣言）があれば、そこから宣言付きで
# scan を走らせ、正解で出し直した判定と一致するかも突き合わせる（Issue #237）。
#
# なぜプロダクトの外にあるか: domains.tsv はドメイン宣言の記法そのものではなく、
# 記法を決めた材料（関数単位の正解まで持つ）。LSP を要し、落ちたら CI が止まる検査には
# 混ぜない。
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
readonly BIN="$PWD/target/release/dryguard"

WORK="$(mktemp -d)"
readonly WORK
trap 'rm -rf "$WORK"' EXIT

"$BIN" scan "$SRC" --format json --explain > "$WORK/scan.json"

# 宣言付きの scan。設定はカレントディレクトリから読むので、置き方のディレクトリで走らせる。
# 宣言が無い置き方では空の候補を置き、突き合わせを飛ばしたことを出力に残す
if [ -f "$LAYOUT/dryguard.toml" ]; then
  (cd "$LAYOUT" && "$BIN" scan src --format json --explain) > "$WORK/declared.json"
else
  echo '{"candidate_pairs": null}' > "$WORK/declared.json"
fi

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
  --slurpfile declared_scan "$WORK/declared.json" \
  --arg src "$(cd "$SRC" && pwd)" \
  --arg prefix "$SRC/" \
  --arg layout "$LAYOUT" '
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
    + " / 呼び出し元 \(.caller_lean) -> \(.declared_caller_lean) [\(.caller_remapped)]"
    + " / 呼び出し先 \(.callee_lean) -> \(.declared_callee_lean) [\(.callee_remapped)]"
    + (if .declared_scan == null then "" else " / 宣言で走らせた scan: \(.declared_scan)" end);

  def reason($pair; $signal): $pair.reasons[] | select(.signal == $signal);

  def jaccard($a; $b):
    ($a | map(select(. as $x | $b | index($x)))) as $shared
    | ($shared | length) / (($a + $b | unique) | length);

  # 呼び出し元・呼び出し先の分布を、正解のドメインへ引き直した傾き。
  # 分布はディレクトリごとにしか出ないので、そのディレクトリ直下のファイルが
  # 正解で 1 つのドメインにまとまるときだけ引き直せる
  def remapped($table; $signal; $side_a; $side_b):
    if $signal.value.status != "measured" then {lean: "neither", remapped: "not-measured"}
    else
      ([$signal.value[$side_a][].domain | directory_domains($table; .)]) as $dirs_a
      | ([$signal.value[$side_b][].domain | directory_domains($table; .)]) as $dirs_b
      | if ($dirs_a + $dirs_b | any(length != 1)) then {lean: "neither", remapped: "mixed-directory"}
        else
          jaccard($dirs_a | map(.[0]) | unique; $dirs_b | map(.[0]) | unique) as $overlap
          | {lean: (if $overlap >= $signal.threshold then "toward-extract"
                    else "toward-do-not-extract" end),
             remapped: "remapped"}
        end
    end;

  def placement($import; $distance):
    if $import == "toward-extract" then "same"
    elif $import == "neither" then "undecidable"
    elif $distance == "toward-do-not-extract" then "separate"
    else "undecidable" end;

  # 呼び出し元と呼び出し先の観測を 1 つにまとめる（observed_domain_match_of の写し）
  def observed($caller; $callee):
    [$caller, $callee] | map(select(. != "neither")) | unique
    | if length == 0 then "none"
      elif length == 2 then "undecidable"
      elif .[0] == "toward-extract" then "same"
      else "separate" end;

  def domain_match($placement; $observed):
    if $observed == "none" then $placement
    elif $observed == "undecidable" then "undecidable"
    elif $observed == "same" then
      (if $placement == "separate" then "undecidable" else "same" end)
    else
      (if $placement == "same" then "undecidable" else "separate" end)
    end;

  def verdict($similar; $import; $distance; $signature; $caller; $callee):
    placement($import; $distance) as $placement
    | domain_match($placement; observed($caller; $callee)) as $match
    | if ($similar | not) then "REVIEW"
      elif $match == "separate" then "DO-NOT-EXTRACT"
      elif $match == "undecidable" then "REVIEW"
      elif $signature == "toward-do-not-extract" then "REVIEW"
      elif $signature == "toward-extract" then "EXTRACT-CANDIDATE"
      elif $placement == "same" then "EXTRACT-CANDIDATE"
      else "REVIEW" end;


  def pair_key($pair; $root):
    "\($pair.location_a.file | ltrimstr($root)):\($pair.location_a.line) <-> \($pair.location_b.file | ltrimstr($root)):\($pair.location_b.line)";

  ($declared_scan[0].candidate_pairs // null) as $declared_pairs
  | (if $declared_pairs == null then null
     else $declared_pairs | map({key: pair_key(.; "src/"), value: .verdict}) | from_entries end)
    as $declared_verdicts
  |

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
     | reason($pair; "callee-domain-overlap") as $callee
     | (if $domain_a == $domain_b then "toward-extract" else "toward-do-not-extract" end)
       as $declared_distance
     | remapped($table; $caller; "callers_a"; "callers_b") as $declared_caller
     | remapped($table; $callee; "callees_a"; "callees_b") as $declared_callee
     | pair_key($pair; $prefix) as $key
     | {
         key: $key,
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
         callee_lean: $callee.lean,
         declared_callee_lean: $declared_callee.lean,
         callee_remapped: $declared_callee.remapped,
         callee_misfires: ($callee.lean != "neither" and $declared_callee.remapped == "remapped"
                           and $callee.lean != $declared_callee.lean),
         remappable: ($declared_caller.remapped != "mixed-directory"
                      and $declared_callee.remapped != "mixed-directory"),
         verdict: $pair.verdict,
         replayed: verdict($similar; $import; $distance.lean; $signature; $caller.lean; $callee.lean),
         declared: verdict($similar; $import; $declared_distance; $signature;
                           $declared_caller.lean; $declared_callee.lean),
         declared_scan: (if $declared_verdicts == null then null
                         else ($declared_verdicts[$key] // "missing") end)
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
    "呼び出し先ドメインが外れた\t\($rows | map(select(.callee_misfires)) | length)",
    "呼び出し先を引き直せなかった（ディレクトリに別ドメインが同居）\t\($rows | map(select(.callee_remapped == "mixed-directory")) | length)",
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
    ($rows | map(select((.distance_misfires or .caller_misfires or .callee_misfires)
                        and .verdict == .declared))
     | .[] | row_text),
    "",
    if $declared_verdicts == null then
      "宣言で走らせた scan\t\($layout)/dryguard.toml が無いので突き合わせていない"
    else
      ($rows | map(select(.remappable))) as $comparable
      | ($comparable | map(select(.declared_scan != .declared))) as $mismatched
      | "宣言で走らせた scan の候補ペア\t\($declared_pairs | length)",
        "突き合わせたペア（呼び出し元・呼び出し先を引き直せたもの）\t\($comparable | length)",
        "正解で出し直した判定と食い違うペア\t\($mismatched | length)",
        "引き直せず突き合わせなかったペア\t\($rows | map(select(.remappable | not)) | length)",
        ($mismatched[] | "食い違い: " + row_text),
        ($rows | map(select(.remappable | not)) | .[]
         | "突き合わせなかった（宣言で走らせた scan: \(.declared_scan) / 正解で出し直した判定: \(.declared)）: \(.a) <-> \(.b)")
    end
  ' "$WORK/scan.json" | tee "$WORK/report.tsv"

# 食い違いがあれば落とす。宣言の実装が正解と違う判定を出している
if grep -q '^正解で出し直した判定と食い違うペア'$'\t''[1-9]' "$WORK/report.tsv"; then
  echo "probe: 宣言で走らせた scan が、正解で出し直した判定と食い違う" >&2
  exit 1
fi
