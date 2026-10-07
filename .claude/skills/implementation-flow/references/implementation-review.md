# 実装の検証の観点

差分を検証するサブエージェントに、**Issue の本文・計画・`git diff origin/main...HEAD`・この観点**を
渡す。サブエージェントはコードを書かず、指摘だけを返す。

**観点は `rules/*.md` の節と、それに対応する `harness/records/TEMPLATE.md`「分類の語彙」から
引く**（移植元の観点を並べない理由は `plan-review.md` と同じ）。各項目の後ろの分類で
`bash harness/records/count.sh` の再発を引ける。所有権・借用・`Result` / `?` など
Rust 共通の観点はプラグインの `coding-standards` が持つので、ここには並べない。

**`unverified-claim` / `claim-staleness` はここに置かない。** 対象は差分ではなく PR 本文・
Issue コメントなので、`SKILL.md` の 7 で本文を書く側が見る。

## シグナルと判定

- 取れなかったシグナルを 0.0 や中立で埋めていないか。逆に、答えに効かない材料の欠けまで
  「測れない」にしていないか（`rules/architecture.md`「取れなかったシグナルを既定値で埋めない」
  「どこまでを「取れなかった」に数えるか」。`missing-signal`）
- 過剰なフォールバック・弾き方で、取れていた値まで落としていないか（`over-guard`）
  **新しい関門・分岐・問い合わせを足した差分には、足す前（`origin/main`）に答えが出ていた
  入力を 1 件挙げ、足した後も同じ答えが出ることを確かめたか。** 無条件に足した問い合わせが、
  その口を持たないサーバや、対象の無いファイルにまで走っていないか（pr-158 指摘 10 / 11 / 13、
  pr-191 指摘 3、pr-199 指摘 6 / 9、pr-308 指摘 4 が同じ形）。挙げられる入力が無いなら、
  差分に回帰テストが無いことを指摘する
- 許可リスト・拒否リストの判定で、**一覧を空にしたとき安全側（偽陰性）に倒れるか**。
  倒れ方の違う 2 箇所で同じ一覧を使い回していないか（`rules/coding.md`
  「列挙で判定を組むときは、漏れの倒れる向きを選ぶ」。`enum-omission-direction`）
- 判定が `classification` の外へ漏れていないか。閾値の既定値が 2 箇所に無いか
  （`rules/architecture.md`「判定は 1 箇所にだけ置く」。`verdict-placement`）
- ファイル単位で決まる答えを、チャンクごとに問い合わせていないか
  （`rules/architecture.md`「ファイル単位で決まる答えをチャンク単位で問い合わせない」。
  `query-granularity`）

## ステージの責務

- 依存の向きと I/O の置き場所（`rules/architecture.md`「依存方向のルール（必須要件）」、
  `rules/coding.md`「禁止事項」「I/O を持ってよい場所」。`layer-dependency`）
- 置いた場所が、依存の向きは壊さないまま責務の表とずれていないか
  （`rules/architecture.md`「3 ステージのパイプライン」。`stage-responsibility`）
- 不要な `pub`・テストのためだけの口が無いか（`rules/architecture.md`「モジュールの公開 API」。
  `module-api`）

## 型とエラー

- エラー型が原因ごとにバリアントを分けているか（`rules/coding.md`
  「エラー型は原因ごとにバリアントを分ける」。`error-variant`）
- 制約を持つ値が、生成時に検証されているか（`rules/coding.md`
  「生成時に検証し、不正な値を存在させない」。`smart-constructor`）
- 矛盾した状態が作れないか。`match` に `_` の受け皿を置いていないか（`rules/coding.md`
  「不正な状態を型で表現できなくする」。`illegal-state`）
- 仕様上決まった語彙を `String` / `f64` のまま持っていないか（`rules/coding.md`
  「値の語彙を型で閉じる」。`type-vocabulary`）

## 名前とコメント

- 名前が約束することと、やること・返すものが一致しているか（`rules/naming.md`
  「名前と実体を一致させる」。`naming-mismatch`）
- `rules/naming.md`「このツールの語彙を固定する」の表の語を使っているか。新しい語を
  足したなら表にも足したか（`naming-vocabulary`）
- コメントが doc と Why / Why not に絞られ、doc の記述（`# Errors` の条件・リンク先）が
  実装と合っているか（`rules/coding.md`「コメントは doc と Why / Why not に絞る」。`comment-mismatch`）

## テスト

- 差分の中心と、エラーの各バリアントの経路にテストがあるか（`rules/testing.md`
  「振る舞いをテストする」。`test-coverage-gap`）
- **その assert を通したまま実装を壊せないか。** 疑わしいものは 1 件だけ壊して走らせたか
  （`rules/testing.md`「assert は「落ちうるか」で見る」。`test-assert-weak`）
- モックを使っていないか（`rules/testing.md`「モックは使わない」。`test-mock`）

## 重複

- 同じ処理が 2 箇所に現れていないか（テスト以外。`duplication-logic`）

## 規約文書との同期

- 差分で変えた関数名・語の定義を、`rules/*.md` が指したままになっていないか
  （`AGENTS.md`「実装を変えたら、rules/ 配下の記述も同じ PR で出し直す」。`rule-staleness`）
