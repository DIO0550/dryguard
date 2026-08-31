import type { User } from "../billing/user";

export function labelImported(value: User): string {
  return String(value.invoiceId);
}
