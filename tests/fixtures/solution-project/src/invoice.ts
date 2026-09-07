import { applyDiscount } from "./discount";
import { applyRebate } from "./rebate";

export type Invoice = {
  amount: number;
};

export function invoiceTotal(invoice: Invoice): number {
  return applyDiscount(invoice) + applyRebate(invoice);
}
