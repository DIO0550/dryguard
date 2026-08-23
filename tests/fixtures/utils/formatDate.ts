import { pad } from "./pad";

export function formatDate(value: Date): string {
  const month = pad(value.getMonth());
  return `month-${month}`;
}
