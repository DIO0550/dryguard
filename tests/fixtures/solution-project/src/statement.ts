import { applyDiscount } from "./discount";
import { applyRebate } from "./rebate";
import { Invoice } from "./invoice";

export function statementLine(invoice: Invoice): string {
  return `${applyDiscount(invoice)} / ${applyRebate(invoice)}`;
}
