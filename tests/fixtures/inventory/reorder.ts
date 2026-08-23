import { Stock } from "./stock";

const THRESHOLD = 0.2;

export function reorderAmount(stock: Stock): number {
  const shortage = stock.quantity * (1 - THRESHOLD);
  return Math.max(shortage, 0);
}
