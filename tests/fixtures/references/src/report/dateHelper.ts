import { pad } from "../utils/pad";

export function dateHelper(value: Date): string {
  const label = pad(value.getMonth());
  return `month-${label}`;
}
