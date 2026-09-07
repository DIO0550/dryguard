import { Invoice } from "./invoice";

const RATE = 0.1;

export function applyDiscount(invoice: Invoice): number {
  const discounted = invoice.amount * (1 - RATE);
  return Math.max(discounted, 0);
}
