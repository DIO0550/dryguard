export interface Receipt {
  total: number;
}

export function makeReceipt(): Receipt {
  return { total: 0 };
}
