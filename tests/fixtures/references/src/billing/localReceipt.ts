// shared/receipt.ts の Receipt と同じ綴りで、構造が違う型。
// **共有の Receipt へは代入できる**（`issuedAt` を持つ）ので、annotated.ts は
// 本体を変えずに戻り値だけを注釈できる。
export class Receipt {
  readonly issuedAt = "";
  readonly invoiceId = "";
}

export function buildLocal(): Receipt {
  return new Receipt();
}
