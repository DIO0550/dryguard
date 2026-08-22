import { Invoice } from "../billing/invoice";
import { Shipment } from "../shipping/label";
import { formatCurrency } from "../shared/money";
import { formatDate } from "../shared/dates";

export function renderInvoiceEmail(invoice: Invoice, total: number): string {
  const due = formatDate(invoice.dueAt);
  const amount = formatCurrency(total, "USD");
  return `Invoice ${invoice.id} for ${amount} is due on ${due}.`;
}

export function renderShipmentEmail(shipment: Shipment): string {
  const shipped = formatDate(shipment.shippedAt);
  return `Shipment ${shipment.id} left on ${shipped} via ${shipment.carrier}.`;
}
