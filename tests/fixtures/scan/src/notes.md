# scan のフィクスチャ

`tests/fixtures/scan/` は `codebase` と `pipeline::scan_of` が見るディレクトリツリー。

- `src/` — 走査の対象。`billing` と `inventory` に構造の同じ関数を 1 つずつ置いてある
  （依存先は食い違うので `DO-NOT-EXTRACT` になる）
- `src/shared/adder.ts` — 入れ子になった関数。外側と内側が 1 ペアとして数えられないことを見る
- `src/linked-billing` — `src/billing` へのシンボリックリンク。辿ってしまうと同じ関数を
  2 回数え、循環したツリーでは走査が終わらない
- `node_modules/` `dist/` — 除外されるディレクトリ。除外が効かないと候補ペアが増える
- このファイル — 拡張子で絞れているかの対照。走査の対象に入っていたらチャンク数が変わる
