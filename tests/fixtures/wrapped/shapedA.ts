// 言い切った型に、このファイルでしか意味を持たない名前を書いた形。
// hover は `const shaped: (v: Shape) => Shape` を返すが、`Shape` は包みの側にしか
// 書かれていない。集め損ねると shapedB.ts の `Shape` と同じ綴りで並ぶ。
interface Shape {
  width: number;
}

export const shaped = ((value: any): any => value) as (v: Shape) => Shape;
