// shapedA.ts と同じ綴りの `Shape` を、別の構造で宣言したもの。
interface Shape {
  label: string;
}

export const shaped = ((value: any): any => value) as (v: Shape) => Shape;
