import { StockItem } from "./stock";

export interface Discrepancy {
  sku: string;
  expected: number;
  counted: number;
}

export function discrepancies(items: StockItem[], counts: Map<string, number>): Discrepancy[] {
  const found: Discrepancy[] = [];
  for (const item of items) {
    const counted = counts.get(item.sku);
    if (counted === undefined || counted === item.quantity) {
      continue;
    }
    found.push({ sku: item.sku, expected: item.quantity, counted });
  }
  return found;
}

export function shrinkage(discrepancyList: Discrepancy[]): number {
  let lost = 0;
  for (const entry of discrepancyList) {
    lost += entry.expected - entry.counted;
  }
  return Math.max(lost, 0);
}
