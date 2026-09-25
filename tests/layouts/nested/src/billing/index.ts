import { Invoice, subtotal } from "./invoices/invoice";
import { balanceDue, statementRows } from "./invoices/statement";
import { taxFor } from "./tax/rate";
import { activeExemptions, isExempt, Exemption } from "./tax/exemption";
import { Payment, amountReceived, paymentsFor } from "./payments/payment";
import { refundablePayments } from "./payments/refund";

export function outstandingFor(invoice: Invoice, payments: Payment[]): number {
  const received = amountReceived(paymentsFor(payments, invoice.id));
  return Math.max(subtotal(invoice) - received, 0);
}

export function customerBalance(invoices: Invoice[], region: string): number {
  return balanceDue(statementRows(invoices, region));
}

export function taxForInvoice(
  invoice: Invoice,
  exemptions: Exemption[],
  region: string,
  asOf: Date,
): number {
  const active = activeExemptions(exemptions, asOf);
  if (isExempt(active, invoice.customerId, region)) {
    return 0;
  }
  return taxFor(subtotal(invoice), region);
}

export function refundableTotal(payments: Payment[], asOf: Date): number {
  return amountReceived(refundablePayments(payments, asOf));
}
