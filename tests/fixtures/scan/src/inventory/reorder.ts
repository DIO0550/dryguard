import { Stock } from "./stock";

export function reorderPoint(stock: Stock, buffer: number): number {
  const point = stock.level * (1 + buffer);
  return Math.max(point, 0);
}
