import { roundToCents } from "../shared/money";

export interface InvoiceLine {
  description: string;
  quantity: number;
  unitPrice: number;
}

export interface Invoice {
  id: string;
  customerId: string;
  lines: InvoiceLine[];
  issuedAt: Date;
  dueAt: Date;
}

export function lineTotal(line: InvoiceLine): number {
  return roundToCents(line.quantity * line.unitPrice);
}

export function invoiceTotal(invoice: Invoice): number {
  let total = 0;
  for (const line of invoice.lines) {
    total += lineTotal(line);
  }
  return roundToCents(total);
}
