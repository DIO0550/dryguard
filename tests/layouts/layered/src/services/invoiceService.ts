import { Invoice } from "../models/invoice";
import { InvoiceRepository } from "../repositories/invoiceRepository";

export class InvoiceService {
  constructor(private readonly repository: InvoiceRepository) {}

  outstandingFor(customerId: string): number {
    let total = 0;
    for (const invoice of this.repository.findByCustomer(customerId)) {
      if (invoice.paid) {
        continue;
      }
      total += invoice.amount;
    }
    return total;
  }

  overdueFor(customerId: string, asOf: Date): Invoice[] {
    const overdue: Invoice[] = [];
    for (const invoice of this.repository.findByCustomer(customerId)) {
      if (invoice.paid || invoice.dueAt.getTime() >= asOf.getTime()) {
        continue;
      }
      overdue.push(invoice);
    }
    return overdue;
  }

  markPaid(id: string): boolean {
    const invoice = this.repository.findById(id);
    if (invoice === null) {
      return false;
    }
    this.repository.save({ ...invoice, paid: true });
    return true;
  }
}
