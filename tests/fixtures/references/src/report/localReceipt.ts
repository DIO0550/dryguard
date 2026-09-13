// billing/localReceipt.ts と同じ綴りで、構造が違う型。
// これがあるので echoed.ts の組は**本当に単一化できない**。
export class Receipt {
  readonly issuedAt = "";
  readonly rowCount = 0;
}

export function buildLocal(): Receipt {
  return new Receipt();
}
