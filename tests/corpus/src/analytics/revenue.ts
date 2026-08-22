import { Invoice, invoiceTotal } from "../billing/invoice";
import { roundToCents } from "../shared/money";

export function totalRevenue(invoices: Invoice[]): number {
  let total = 0;
  for (const invoice of invoices) {
    total += invoiceTotal(invoice);
  }
  return roundToCents(total);
}

export function growthRate(previous: number, current: number): number | null {
  if (previous === 0) {
    return null;
  }
  return (current - previous) / previous;
}

export function invoicesIssuedAfter(invoices: Invoice[], since: Date): Invoice[] {
  const recent: Invoice[] = [];
  for (const invoice of invoices) {
    if (invoice.issuedAt.getTime() <= since.getTime()) {
      continue;
    }
    recent.push(invoice);
  }
  return recent;
}
