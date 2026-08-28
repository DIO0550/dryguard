# 依存の解決

**入口は 2 つある。** どちらも素で叩かない。

| 対象 | 入口 | cooldown の指定 |
|---|---|---|
| Rust | `harness/deps/resolve.sh` | `.cargo/config.toml` の `global-min-publish-age`（14 日） |
| JavaScript | `harness/deps/resolve-node.sh` | `pnpm-workspace.yaml` の `minimumReleaseAge`（5 日 = 7200 分） |

日数が違うのは揃えそこねているからではなく、**まだどちらも実測で決めていない**ため。
片方を変えるときはもう片方も見る。

## Rust

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

## JavaScript

対象は **CI が Stage 2 のテストで起動する LSP サーバ**（typescript-language-server と
typescript）だけ。このリポジトリに JavaScript のコードは無い。

`pnpm-lock.yaml` を作り直すときは **`harness/deps/resolve-node.sh` を通す**。

```bash
bash harness/deps/resolve-node.sh install --lockfile-only
bash harness/deps/resolve-node.sh update
bash harness/deps/resolve-node.sh add -D <パッケージ>
```

**lock から入れるだけなら通さなくてよい。** `pnpm install --frozen-lockfile` は解決を伴わず、
掴むものが lock で決まっているので cooldown の出番が無い。CI がやっているのはこちら。

### なぜ版を確かめるか

`minimumReleaseAge` は **pnpm 10.16.0 で入った**。それより前の版は**この設定を警告なく無視する**。
cargo 側で stable が `min-publish-age` を無言で捨てるのと同じ形なので、同じように入口で落とす。

### typescript の版を固定している理由

typescript-language-server は tsserver を包むだけで、**TypeScript 本体を同梱しない**。
そして **TypeScript 7 は `tsserver.js` を配らない**ので、範囲で解決させると
`initialize` が `Could not find a valid TypeScript installation` で失敗する
（[#25](https://github.com/DIO0550/dryguard/issues/25) で実際に踏んだ）。

### 前提として引き受けること

- **lock が固定し、cooldown は入口を絞るだけ。** 毎回同じものが入るのは lock のおかげで、
  cooldown は lock を作り直す瞬間にしか効かない
- **CI が使う pnpm 自体は lock の外**。版と SHA-256 を `.github/workflows/rust.yml` で固定する
