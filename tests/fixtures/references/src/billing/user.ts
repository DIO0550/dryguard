export interface User {
  readonly invoiceId: string;
}

export function labelUser(value: User): string {
  return String(value.invoiceId);
}
