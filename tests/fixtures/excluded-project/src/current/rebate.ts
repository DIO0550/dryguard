import { Invoice } from "./invoice";

const RATE = 0.2;

export function applyRebate(invoice: Invoice): number {
  const rebated = invoice.amount * (1 - RATE);
  return Math.max(rebated, 0);
}
