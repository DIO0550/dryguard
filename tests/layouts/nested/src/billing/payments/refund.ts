import { Payment } from "./payment";

const REFUND_WINDOW_DAYS = 30;
const MS_PER_DAY = 24 * 60 * 60 * 1000;

export function isRefundable(payment: Payment, asOf: Date): boolean {
  const elapsed = (asOf.getTime() - payment.receivedAt.getTime()) / MS_PER_DAY;
  return !payment.refunded && elapsed <= REFUND_WINDOW_DAYS;
}

export function refundablePayments(payments: Payment[], asOf: Date): Payment[] {
  const refundable: Payment[] = [];
  for (const payment of payments) {
    if (!isRefundable(payment, asOf)) {
      continue;
    }
    refundable.push(payment);
  }
  return refundable;
}
