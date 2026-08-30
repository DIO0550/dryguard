# dryguard

**構造の似たコードが偶発的な重複かどうかを、理由付きで判定する CLI。**

AI コーディングエージェントは、テキスト的に似たコードを見つけると安易に共通化する。
しかし「テキストが似ている」と「変更理由が同じ」は別物で、偶発的な重複
(incidental duplication) を共通化すると間違った抽象化が生まれる。

このツールは、似たコードペアを検出するだけでなく、**型・依存・参照の意味情報から
「共通化してよいか / すべきでないか」を理由付きで分類する**。

| ラベル | 意味 |
|---|---|
| `EXTRACT-CANDIDATE` | 構造・型・依存ドメインが一致。共通化してよい |
| `DO-NOT-EXTRACT` | 構造は似ているが依存先・呼び出し元のドメインが別。偶発的重複 |
| `REVIEW` | 中間ケース。人間の判断が要る |

## 差別化ポイント

1. **「共通化するな」を言える。** 検出だけのツールは重複を報告するだけで、共通化の是非は判断しない
2. **判断に理由が付く。** ML・埋め込みを使わずルールベースにするのは、説明可能性がこのツールの価値そのものだから
3. **AI エージェント向けの出力。** 人間のリファクタリング支援ではなく、エージェントの自己修正ループに組み込む JSON が一級市民

## 状態

**Phase 2（途中）。** 引数の受け口・関数チャンクの切り出し・構造類似度・
依存モジュールの重なり・モジュール距離に加えて、**型シグネチャが単一化できるかと
呼び出し元がどのドメインから来ているか**（Stage 2）を判定へ渡している。
5 つのシグナルを統合して 3 ラベルと、シグナルごとの理由・提案を出す。閾値はハードコード。

意味情報は **LSP サーバを子プロセスで起動し、自前の JSON-RPC で握手して
hover / references を尋ねる**。**サーバを使えない環境でも判定は動き**、
そのときは Stage 1 のシグナルだけで判定して「測れない (LSP サーバを使えない)」を
出力に出す。今つないでいるのは `compare` だけで、`scan` の候補ペアへの配線はまだ。

Stage 1 のうち**チャンクの切り出しと import の収集は tree-sitter** に載っている。
`scan` はコードベース全体を総当たりで比べ、**ファイルの解析とペアの比較は rayon で
並列に回す**（出力の並びは逐次で回したときと変わらない）。

トークン列の長さから出る**上限で、閾値に届かないペアの突き合わせを省く**。
省いた数は走査した量に出る。上限は実際の類似度を下回らないので、
**候補ペアは省かないときと 1 組も変わらない**。候補を落とす種類の事前フィルタ
（MinHash/LSH・ブルームフィルタ）はまだ無く、比較の数も n² のまま。

進め方は**縦切り（walking skeleton）**で、各ステージを順に完成させるのではなく
全ステージを雑に貫通させてから厚くする。計画は [`docs/dryguard-plan.md`](docs/dryguard-plan.md)、
分割した作業は [Issues](https://github.com/DIO0550/dryguard/issues)。

## アーキテクチャ

```
[Stage 1: 候補抽出]      [Stage 2: 意味情報収集]   [Stage 3: 分類]
 syntax                   semantics                classification
 tree-sitter              LSP                      ルールエンジン
  ├ 関数/メソッド単位で    ├ hover (型シグネチャ)     ├ シグナル統合
  │ チャンク化            ├ callHierarchy           ├ ドメイン距離判定
  ├ AST正規化             ├ references              └ 理由付きラベル
  └ 類似ペア列挙          └ documentSymbol
```

## 使い方

```bash
cargo run -- compare <locA> <locB>   # 特定の 2 関数を比較 (file:line)
cargo run -- scan [path]             # コードベース全体をスキャン (既定は .)

オプション:
  --lang ts|auto
  --format text
  --threshold <0-1>
  --explain                          # 判定根拠のシグナル値を全表示
  --fail-on do-not-extract           # 非推奨ペアがあれば exit 1
```

`scan` が見るのは `.ts` と `.tsx` で、`node_modules` / `dist` / `build` / `target` / `.git` は
降りない。**読むときの grammar は拡張子で選ぶ**（JSX は TypeScript の grammar では読めず、
型アサーション `<T>value` は TSX の grammar では JSX に見えるので、片方で兼ねられない）。
読めなかったファイルと構文エラーで切り出せなかった関数は、
**候補ペアの後ろに一覧で出す**（黙って飛ばすと、対象だったのか除外されたのかが分からない）。

`check --diff` と `--format json` は後のフェーズで足す。
**まだ動かないコマンド・選択肢はヘルプに並べない**方針のため、現時点では出ない。

## 開発

```bash
cargo fmt --check                           # フォーマット検査（CI と同じ）
cargo clippy --all-targets -- -D warnings   # Lint（テストコードも対象）
cargo test                                  # テスト
bash harness/githooks/pre-push              # push 前の検査をまとめて
```

実装規約は [`AGENTS.md`](AGENTS.md) と [`rules/`](rules/)。
規約を強制する仕組みは [`harness/`](harness/) と `.github/workflows/`。

### Dev Container

`.devcontainer/` に設定がある。VS Code のコマンドパレットから
「Dev Containers: Reopen in Container」で起動する。

## ライセンス

[MIT](LICENSE)
