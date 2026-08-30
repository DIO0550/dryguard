import { applyDiscount } from "./discount";
import { Invoice } from "./invoice";

export function statementLine(invoice: Invoice): string {
  return `total: ${applyDiscount(invoice)}`;
}
