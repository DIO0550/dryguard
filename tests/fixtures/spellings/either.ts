// hover は共用体を書かれた順のまま返す。並びを正規化しないと、
// 同じ型を受ける `reversed` と重ならない。
export function either(value: string | number): void {
  void value;
}
