export class Receipt {
  readonly invoiceId = "";
}

export function buildReceipt(count: number) {
  void count;
  return new Receipt();
}
