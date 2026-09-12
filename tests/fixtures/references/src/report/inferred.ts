export class Receipt {
  readonly rowCount = 0;
}

export function buildReceipt(count: number) {
  void count;
  return new Receipt();
}
