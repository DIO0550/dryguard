import { Invoice, subtotal, unpaidInvoices } from "./invoice";
import { taxFor } from "../tax/rate";

export interface StatementRow {
  invoiceId: string;
  amount: number;
}

export function statementRows(invoices: Invoice[], region: string): StatementRow[] {
  const rows: StatementRow[] = [];
  for (const invoice of unpaidInvoices(invoices)) {
    const net = subtotal(invoice);
    rows.push({ invoiceId: invoice.id, amount: net + taxFor(net, region) });
  }
  return rows;
}

export function balanceDue(rows: StatementRow[]): number {
  let total = 0;
  for (const row of rows) {
    total += row.amount;
  }
  return total;
}
