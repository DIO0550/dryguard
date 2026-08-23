# 依存の解決

`cargo update` / `cargo add` は **`harness/deps/resolve.sh` を通す**。素で叩かない。

```bash
bash harness/deps/resolve.sh update
bash harness/deps/resolve.sh add serde
bash harness/deps/resolve.sh update --manifest-path harness/phase0/chunk-unit-probe/Cargo.toml
```

## なぜ入口を 1 つにするか

悪意あるバージョンは、**`cargo build` の時点で build script と proc macro がローカル実行される**。
掴んだ後に CI で気付いても、コードはもう自分のマシンで走っている。止められるのは**解決の瞬間**だけ。

`.cargo/config.toml` の `global-min-publish-age` が「公開から N 日経っていないバージョンを
使わない」を指定するが、**これは nightly でしか効かない**（`min-publish-age` は 2026-08 時点で
unstable。判定材料の `pubtime` 自体は Rust 1.94.0 で stable）。

stable で走らせても**エラーにも警告にもならず、ただ無保護になる**。
`resolve.sh` は nightly が無ければ落ちるので、この「走ったが守られていない」を作らない。

## 効いていることの確かめ方

期間内のバージョンを名指しで取りにいって、弾かれることを見る。

```bash
bash harness/deps/resolve.sh update -p <crate> --precise <公開間もないバージョン>
```

公開時刻は sparse index から引ける（crates.io の API を叩かなくてよい）。

```bash
curl -s https://index.crates.io/cc/1.4.4 | tail -1 | grep -o '"pubtime":"[^"]*"'
```

## 前提として引き受けること

- **既存の `Cargo.lock` は免除される。** cooldown は入口のフィルタで、既に lock にあるバージョンは
  そのまま通る。導入時点の lock は別途監査する（[#91](https://github.com/DIO0550/dryguard/issues/91) で実施）
- **registry 由来の依存にしか効かない。** git / path 依存は `pubtime` を持たない
- **cooldown は検知しない。** 発見・yank されるまでの時間を稼ぐだけなので、これ単体を拠り所にしない
- **緊急のセキュリティ修正は逃げ道を使う。** `incompatible-publish-age` を `allow` にして
  `cargo update --precise` で取り込む。**使ったことを PR に書く**（黙って抜けると、なぜ期間内の
  バージョンが lock に入ったのかが後から読めない）
