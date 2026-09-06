import { applyDiscount } from "./discount";
import { Invoice } from "./invoice";
import { applyRebate } from "./rebate";

export function statementLine(invoice: Invoice): string {
  return `total: ${applyDiscount(invoice)}`;
}

export function rebateLine(invoice: Invoice): string {
  return `rebate: ${applyRebate(invoice)}`;
}
