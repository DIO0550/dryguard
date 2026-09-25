import { Invoice } from "../models/invoice";

export class InvoiceRepository {
  private readonly invoices = new Map<string, Invoice>();

  save(invoice: Invoice): void {
    this.invoices.set(invoice.id, invoice);
  }

  findById(id: string): Invoice | null {
    const invoice = this.invoices.get(id);
    if (invoice === undefined) {
      return null;
    }
    return invoice;
  }

  findByCustomer(customerId: string): Invoice[] {
    const found: Invoice[] = [];
    for (const invoice of this.invoices.values()) {
      if (invoice.customerId !== customerId) {
        continue;
      }
      found.push(invoice);
    }
    return found;
  }
}
