import { InvoiceService } from "../services/invoiceService";

export interface Response {
  status: number;
  body: unknown;
}

export class InvoiceController {
  constructor(private readonly service: InvoiceService) {}

  getOutstanding(customerId: string): Response {
    const outstanding = this.service.outstandingFor(customerId);
    return { status: 200, body: { customerId, outstanding } };
  }

  getOverdue(customerId: string): Response {
    const overdue = this.service.overdueFor(customerId, new Date());
    return { status: 200, body: overdue };
  }

  postPayment(id: string): Response {
    if (!this.service.markPaid(id)) {
      return { status: 404, body: { error: `invoice ${id} not found` } };
    }
    return { status: 204, body: null };
  }
}
