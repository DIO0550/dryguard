export interface Payment {
  id: string;
  invoiceId: string;
  amount: number;
  receivedAt: Date;
  refunded: boolean;
}

export function paymentsFor(payments: Payment[], invoiceId: string): Payment[] {
  const matched: Payment[] = [];
  for (const payment of payments) {
    if (payment.invoiceId !== invoiceId) {
      continue;
    }
    matched.push(payment);
  }
  return matched;
}

export function amountReceived(payments: Payment[]): number {
  let sum = 0;
  for (const payment of payments) {
    if (payment.refunded) {
      continue;
    }
    sum += payment.amount;
  }
  return sum;
}
