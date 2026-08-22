import { Invoice } from "./invoice";
import { daysBetween } from "../shared/dates";

const RETRY_DELAYS_DAYS = [1, 3, 7, 14];

export function isOverdue(invoice: Invoice, asOf: Date): boolean {
  return asOf.getTime() > invoice.dueAt.getTime();
}

export function overdueInvoices(invoices: Invoice[], asOf: Date): Invoice[] {
  const overdue: Invoice[] = [];
  for (const invoice of invoices) {
    if (!isOverdue(invoice, asOf)) {
      continue;
    }
    overdue.push(invoice);
  }
  return overdue;
}

export function daysOverdue(invoice: Invoice, asOf: Date): number {
  const elapsed = daysBetween(invoice.dueAt, asOf);
  return Math.max(elapsed, 0);
}

export function nextRetryDelay(attempt: number): number | null {
  if (attempt >= RETRY_DELAYS_DAYS.length) {
    return null;
  }
  return RETRY_DELAYS_DAYS[attempt];
}
