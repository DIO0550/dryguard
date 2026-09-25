import { Charge, lateFee, settledTotal } from "../utils/helpers";
import { formatDate, invoiceNumber } from "../utils/format";

export interface Account {
  id: string;
  charges: Charge[];
  credit: number;
}

export function balanceOf(account: Account): number {
  return Math.max(settledTotal(account.charges) - account.credit, 0);
}

export function balanceWithFees(account: Account, monthsLate: number): number {
  const balance = balanceOf(account);
  return balance + lateFee(balance, monthsLate);
}

export function invoiceHeader(account: Account, sequence: number, issuedAt: Date): string {
  const number = invoiceNumber(sequence, issuedAt);
  return `${number} ${account.id} ${formatDate(issuedAt)}`;
}
