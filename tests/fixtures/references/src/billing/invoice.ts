import { applyDiscount } from "./discount";

export type Invoice = {
  amount: number;
};

export function invoiceTotal(invoice: Invoice): number {
  return applyDiscount(invoice);
}
