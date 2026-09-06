import { applyDiscount } from "./discount";

export function invoiceTotal(amount: number): number {
  return applyDiscount(amount);
}
