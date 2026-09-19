// `shared.ts` と**同じ綴りで構造の違う** `Receipt`。共有の `Receipt` へは代入できるが、
// `report.ts` の `Receipt` とは互いに代入できない
export interface Receipt {
  id: string;
  issuedAt: number;
}

export function makeReceipt(): Receipt {
  return { id: "billing", issuedAt: 0 };
}
