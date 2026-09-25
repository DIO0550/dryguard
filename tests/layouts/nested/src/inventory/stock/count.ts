import { StockItem, available } from "./item";

export interface CountResult {
  sku: string;
  counted: number;
}

export function itemsAt(items: StockItem[], warehouseId: string): StockItem[] {
  const here: StockItem[] = [];
  for (const item of items) {
    if (item.warehouseId !== warehouseId) {
      continue;
    }
    here.push(item);
  }
  return here;
}

export function totalAvailable(items: StockItem[]): number {
  let sum = 0;
  for (const item of items) {
    sum += available(item);
  }
  return sum;
}

export function discrepancies(items: StockItem[], counts: CountResult[]): CountResult[] {
  const mismatched: CountResult[] = [];
  for (const count of counts) {
    const item = items.find((candidate) => candidate.sku === count.sku);
    if (item && item.onHand === count.counted) {
      continue;
    }
    mismatched.push(count);
  }
  return mismatched;
}
