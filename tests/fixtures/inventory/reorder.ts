import { Stock } from "./stock";

const THRESHOLD = 5;

export function reorderAmount(stock: Stock): number {
  const shortage = THRESHOLD - stock.quantity;
  return Math.max(shortage, 0);
}
