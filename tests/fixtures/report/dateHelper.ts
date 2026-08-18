import { pad } from "../utils/pad";

export function dateHelper(value: Date): string {
  const day = pad(value.getDate());
  return `day-${day}`;
}
