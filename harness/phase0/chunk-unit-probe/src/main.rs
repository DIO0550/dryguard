//! チャンクの単位を関数からブロックまで落としたときに、何が拾えて何が増えるかを数える。
//!
//! Issue #18（チャンクの単位を決める）の判断材料。`harness/phase0/verify.sh` は
//! **関数単位で切り出したペア**しか見ないので、「関数の一部だけが重複しているケースを
//! 取りこぼしているか」（偽陰性）は数に出ない。そこだけを測る。
//!
//! 使い方:
//!
//! ```text
//! cargo run --manifest-path harness/phase0/chunk-unit-probe/Cargo.toml -- <対象ディレクトリ>
//! ```

use std::collections::BTreeSet;
use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use dryguard::classification::DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD;
use dryguard::syntax::token::TokenSet;
use dryguard::syntax::tree::SyntaxTree;
use tree_sitter::Node;

/// 関数として拾うノードの種別。`src/syntax/chunk.rs` の `CHUNK_KINDS` に合わせてある。
///
/// **合わせてあるだけで、同じ実装ではない**（`verify.sh` が `is_function_header` に
/// 対して取っているのと同じ形）。ずれた場合は関数の数が `verify.sh` の種の数と
/// 食い違うので、突き合わせれば気付ける。
const FUNCTION_KINDS: [&str; 6] = [
    "function_declaration",
    "generator_function_declaration",
    "function_expression",
    "generator_function",
    "arrow_function",
    "method_definition",
];

/// ブロックとして拾うノードの種別。
///
/// 波括弧そのもの（`statement_block`）ではなく**それを持つ文**を採る。`for (..) {` の
/// 見出し行はブロックの意味の一部で、落とすと条件の違う 2 つのループが同じトークン集合になる。
const BLOCK_KINDS: [&str; 7] = [
    "if_statement",
    "for_statement",
    "for_in_statement",
    "while_statement",
    "do_statement",
    "switch_statement",
    "try_statement",
];

/// 比較の単位 1 つ分。
struct Unit {
    path: PathBuf,
    start_line: usize,
    end_line: usize,
    token_count: usize,
    tokens: TokenSet,
    /// このブロックが属する関数の添字。関数自身の場合は自分の添字。
    owner: usize,
}

impl Unit {
    /// `compare` にそのまま渡せる形の位置。
    fn location(&self) -> String {
        format!("{}:{}", self.path.display(), self.start_line)
    }

    fn lines(&self) -> usize {
        self.end_line - self.start_line + 1
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let target = env::args()
        .nth(1)
        .ok_or("対象ディレクトリを指定してください")?;
    let threshold = DEFAULT_STRUCTURAL_SIMILARITY_THRESHOLD;

    let mut functions: Vec<Unit> = Vec::new();
    let mut blocks: Vec<Unit> = Vec::new();
    for path in typescript_files_of(Path::new(&target))? {
        let source = fs::read_to_string(&path)?;
        collect_units(&path, &source, &mut functions, &mut blocks)?;
    }

    if functions.len() < 2 {
        return Err("比較できる関数が 2 件未満です".into());
    }

    println!("=== 数えたもの ===");
    println!("関数: {} 件", functions.len());
    println!("ブロック: {} 件", blocks.len());
    println!("関数単位のペア: {} 件", pair_count(functions.len()));
    println!(
        "ブロック単位のペア: {} 件（関数 + ブロックをすべて比較したとき）",
        pair_count(functions.len() + blocks.len())
    );
    println!();

    println!("=== 閾値 {threshold} を超えたペア ===");
    let similar_function_pairs = similar_pair_count_of(&functions, threshold);
    let similar_block_pairs = similar_block_pairs_of(&blocks, threshold);
    println!("関数単位: {similar_function_pairs} 件");
    println!(
        "ブロック単位で新たに増える分: {} 件（別々の関数に属するブロックどうし）",
        similar_block_pairs.len()
    );
    println!();

    println!("=== 関数単位が取りこぼし、ブロック単位が拾うペア ===");
    let missed = missed_pairs_of(&functions, &blocks, threshold);
    if missed.is_empty() {
        println!("なし");
    }
    for (left, right, function_similarity, block_similarity) in &missed {
        println!(
            "{} <-> {}\n  関数単位 {:.2} / ブロック単位 {:.2}\n  ブロック: {} <-> {}",
            functions[left.owner].location(),
            functions[right.owner].location(),
            function_similarity,
            block_similarity,
            left.location(),
            right.location(),
        );
    }
    println!();

    println!("=== ブロックの大きさ ===");
    print_size_distribution(&blocks, "ブロック");
    print_size_distribution(&functions, "関数");

    Ok(())
}

/// 対象ディレクトリ以下の TypeScript ファイル。パス順に並べる。
fn typescript_files_of(root: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    if !root.is_dir() {
        return Err(format!("対象ディレクトリがありません: {}", root.display()).into());
    }

    let mut files = BTreeSet::new();
    let mut pending = vec![root.to_path_buf()];

    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            if path.is_dir() {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                if name == "node_modules" || name == ".git" || name == "dist" || name == "build" {
                    continue;
                }
                pending.push(path);
                continue;
            }
            let extension = path.extension().unwrap_or_default().to_string_lossy();
            if extension == "ts" || extension == "tsx" {
                files.insert(path);
            }
        }
    }

    Ok(files.into_iter().collect())
}

/// 1 ファイル分の関数とブロックを集める。ブロックには属する関数の添字を持たせる。
///
/// どの関数にも属さないブロック（トップレベルの `if` など）は落とす。比較の単位を
/// 落とすかどうかの話なので、**関数の中にあるブロック**だけが対象になる。
fn collect_units(
    path: &Path,
    source: &str,
    functions: &mut Vec<Unit>,
    blocks: &mut Vec<Unit>,
) -> Result<(), Box<dyn Error>> {
    let tree = SyntaxTree::from_typescript(source)?;
    let nodes = tree.named_descendants();

    let function_nodes: Vec<Node<'_>> = nodes
        .iter()
        .copied()
        .filter(|node| FUNCTION_KINDS.contains(&node.kind()))
        .collect();

    let offset = functions.len();
    for (index, node) in function_nodes.iter().enumerate() {
        if let Some(unit) = unit_of(path, source, *node, offset + index) {
            functions.push(unit);
        }
    }

    for node in nodes.iter().copied() {
        if !BLOCK_KINDS.contains(&node.kind()) {
            continue;
        }
        let Some(owner) = innermost_owner_of(&function_nodes, node) else {
            continue;
        };
        if let Some(unit) = unit_of(path, source, node, offset + owner) {
            blocks.push(unit);
        }
    }

    Ok(())
}

/// そのノードを覆っている、もっとも内側の関数の添字。どれにも覆われていなければ `None`。
fn innermost_owner_of(function_nodes: &[Node<'_>], node: Node<'_>) -> Option<usize> {
    function_nodes
        .iter()
        .enumerate()
        .filter(|(_, function)| {
            function.byte_range().start <= node.byte_range().start
                && node.byte_range().end <= function.byte_range().end
        })
        .min_by_key(|(_, function)| function.byte_range().len())
        .map(|(index, _)| index)
}

/// ノード 1 つ分の比較単位。トークンが 1 つも無いときは `None`。
///
/// ノードが覆っているテキストではなく**行ごと**採る。`src/syntax/chunk.rs` の
/// `source_of_lines` に合わせるためで、合わせないと `export function f() {` の
/// `export` が入らず、**同じペアでも `compare` と違う類似度が出る**。
fn unit_of(path: &Path, source: &str, node: Node<'_>, owner: usize) -> Option<Unit> {
    let start_line = node.start_position().row + 1;
    let end_line = node.end_position().row + 1;
    let text = source_of_lines(source, start_line, end_line);
    let tokens = TokenSet::from_source(&text)?;

    Some(Unit {
        path: path.to_path_buf(),
        start_line,
        end_line,
        token_count: dryguard::syntax::token::tokens_of(&text).len(),
        tokens,
        owner,
    })
}

/// 1 始まりの行範囲のソース。行の区切りは改行 1 文字。
fn source_of_lines(source: &str, start_line: usize, end_line: usize) -> String {
    source
        .lines()
        .skip(start_line - 1)
        .take(end_line + 1 - start_line)
        .collect::<Vec<&str>>()
        .join("\n")
}

fn pair_count(count: usize) -> usize {
    count * count.saturating_sub(1) / 2
}

/// 閾値に届いた関数ペアの数。
fn similar_pair_count_of(functions: &[Unit], threshold: dryguard::threshold::Threshold) -> usize {
    let mut count = 0;
    for (index, left) in functions.iter().enumerate() {
        for right in &functions[index + 1..] {
            if left.tokens.jaccard(&right.tokens).is_at_least(threshold) {
                count += 1;
            }
        }
    }
    count
}

/// 別々の関数に属するブロックどうしで、閾値に届いたペア。
fn similar_block_pairs_of(
    blocks: &[Unit],
    threshold: dryguard::threshold::Threshold,
) -> Vec<(&Unit, &Unit)> {
    let mut pairs = Vec::new();
    for (index, left) in blocks.iter().enumerate() {
        for right in &blocks[index + 1..] {
            if left.owner == right.owner {
                continue;
            }
            if left.tokens.jaccard(&right.tokens).is_at_least(threshold) {
                pairs.push((left, right));
            }
        }
    }
    pairs
}

/// 関数単位では閾値に届かないが、中のブロックどうしなら届く関数ペア。
///
/// 「関数の一部だけが重複しているケースを取りこぼしているか」がこれ。
/// 1 つの関数ペアにつき、もっとも似ているブロックの組を 1 件だけ返す。
fn missed_pairs_of<'unit>(
    functions: &[Unit],
    blocks: &'unit [Unit],
    threshold: dryguard::threshold::Threshold,
) -> Vec<(&'unit Unit, &'unit Unit, f64, f64)> {
    let mut missed = Vec::new();

    for (index, left) in functions.iter().enumerate() {
        for (offset, right) in functions[index + 1..].iter().enumerate() {
            let function_similarity = left.tokens.jaccard(&right.tokens);
            if function_similarity.is_at_least(threshold) {
                continue;
            }

            let right_index = index + 1 + offset;
            let best = blocks
                .iter()
                .filter(|block| block.owner == index)
                .flat_map(|left_block| {
                    blocks
                        .iter()
                        .filter(|block| block.owner == right_index)
                        .map(move |right_block| {
                            (
                                left_block,
                                right_block,
                                left_block.tokens.jaccard(&right_block.tokens),
                            )
                        })
                })
                .filter(|(_, _, similarity)| similarity.is_at_least(threshold))
                .max_by(|left_pair, right_pair| {
                    left_pair.2.value().total_cmp(&right_pair.2.value())
                });

            if let Some((left_block, right_block, block_similarity)) = best {
                missed.push((
                    left_block,
                    right_block,
                    function_similarity.value(),
                    block_similarity.value(),
                ));
            }
        }
    }

    missed
}

/// 単位の大きさの分布。短い単位ほど構文語がトークン集合を占める（Issue #86）ので、
/// 単位を落とすとその影響がどれだけ増えるかをここで見る。
fn print_size_distribution(units: &[Unit], label: &str) {
    if units.is_empty() {
        println!("{label}: なし");
        return;
    }

    let mut token_counts: Vec<usize> = units.iter().map(|unit| unit.token_count).collect();
    token_counts.sort_unstable();
    let mut line_counts: Vec<usize> = units.iter().map(Unit::lines).collect();
    line_counts.sort_unstable();

    println!(
        "{label}: 行数 中央値 {} (最小 {} / 最大 {}) / トークン数 中央値 {} (最小 {} / 最大 {})",
        line_counts[line_counts.len() / 2],
        line_counts[0],
        line_counts[line_counts.len() - 1],
        token_counts[token_counts.len() / 2],
        token_counts[0],
        token_counts[token_counts.len() - 1],
    );
}
