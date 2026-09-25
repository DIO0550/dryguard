export interface InvoiceLine {
  description: string;
  unitPrice: number;
  quantity: number;
}

export interface Invoice {
  id: string;
  customerId: string;
  issuedAt: Date;
  dueAt: Date;
  lines: InvoiceLine[];
  paid: boolean;
}

export function lineAmount(line: InvoiceLine): number {
  return line.unitPrice * line.quantity;
}

export function subtotal(invoice: Invoice): number {
  let sum = 0;
  for (const line of invoice.lines) {
    sum += lineAmount(line);
  }
  return sum;
}

export function unpaidInvoices(invoices: Invoice[]): Invoice[] {
  const unpaid: Invoice[] = [];
  for (const invoice of invoices) {
    if (invoice.paid) {
      continue;
    }
    unpaid.push(invoice);
  }
  return unpaid;
}
