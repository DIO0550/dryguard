import { dateHelper } from "./dateHelper";
import { formatDate } from "../utils/formatDate";

export function monthlyLabel(value: Date): string {
  return `${formatDate(value)} ${dateHelper(value)}`;
}
