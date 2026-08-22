import { Invoice, invoiceTotal } from "./invoice";
import { roundToCents } from "../shared/money";

const STANDARD_RATE = 0.1;
const BULK_THRESHOLD = 10000;
const MID_THRESHOLD = 1000;

export function tieredRate(total: number): number {
  if (total >= BULK_THRESHOLD) {
    return 0.2;
  }
  if (total >= MID_THRESHOLD) {
    return 0.15;
  }
  return STANDARD_RATE;
}

export function applyDiscount(invoice: Invoice): number {
  const total = invoiceTotal(invoice);
  const discounted = total * (1 - tieredRate(total));
  return Math.max(roundToCents(discounted), 0);
}
