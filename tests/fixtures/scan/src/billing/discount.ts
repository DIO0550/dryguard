import { Invoice } from "./invoice";

export function applyDiscount(invoice: Invoice, rate: number): number {
  const discounted = invoice.amount * (1 - rate);
  return Math.max(discounted, 0);
}
