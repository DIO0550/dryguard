# 開発手順の判例

## `plan`

- Issue の表を起点に計画し、付け替え後に完了条件を grep したところ、表にない変更箇所が見つかった。
  調べる範囲と完了条件を決め、計画する時点で対象を数え直す。[pr-72 指摘 1](../records/pr-72.md)
- 依存先集合の置き場所を計画した後、`docs/dryguard-plan.md` の Stage 1 に各チャンクが持つと書かれていることに気づいた。
  置き場所の判断には計画書の該当ステージを照合する。[pr-74 指摘 1](../records/pr-74.md)
- `echo hook-canary` が通ったことを層 3 の不発として報告したが、起動時には canary が未配線だった。
  判定には起動時点の `PreToolUse` にある `hook-canary.sh` の配線が必要。`pre-push-check.sh` が見るのは push の検査で、
  その配線だけでは層 3 の canary による判定はできない。[pr-95 指摘 4](../records/pr-95.md)、[pr-97 指摘 4](../records/pr-97.md)、[pr-138 指摘 4](../records/pr-138.md)

## `unverified-claim`

レビュー返信で PR の `updated_at` を本文の更新時刻として引用した。観測した値はレビューコメントが付いた時刻だった。
一次情報の値を引用する前に、その値が主張したいイベントを表すかまで確かめる。[pr-194 指摘 9](../records/pr-194.md)

## `claim-staleness`

レビュー対応で stderr の文言を変えた後、PR 本文の出力例が前の文言のまま残った。別の回では、修正コミットで回帰テストを
足した後も PR 本文のテスト表が更新されていなかった。push 前に、変更した出力とテストが本文のどの記述に効くかまで辿る。
[pr-129 指摘 9](../records/pr-129.md)、[pr-138 指摘 5](../records/pr-138.md)

## `rule-staleness`

`project root` の探索をファイルごとに変えたとき、`rules/naming.md` の語彙表には前の定義が残った。
実装が扱う型の範囲を広げた別の回では、`rules/architecture.md` が消えたヘルパーを指していた。
規約が挙げる語や関数名を変更後の定義に当て、同じ PR で同期する。
[pr-155 指摘 3](../records/pr-155.md)、[pr-186 指摘 12](../records/pr-186.md)
