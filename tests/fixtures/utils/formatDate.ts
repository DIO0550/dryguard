import { pad } from "./pad";

export function formatDate(value: Date): string {
  const month = pad(value.getMonth() + 1);
  return `${value.getFullYear()}-${month}`;
}
