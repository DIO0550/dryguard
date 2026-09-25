import { StockItem } from "../stock/item";

export function itemValue(item: StockItem): number {
  return item.unitCost * item.onHand;
}

export function inventoryValue(items: StockItem[]): number {
  let sum = 0;
  for (const item of items) {
    sum += itemValue(item);
  }
  return sum;
}

export function writeOffs(items: StockItem[]): StockItem[] {
  const written: StockItem[] = [];
  for (const item of items) {
    if (!item.discontinued) {
      continue;
    }
    written.push(item);
  }
  return written;
}
