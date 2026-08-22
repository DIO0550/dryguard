import { StockItem } from "../inventory/stock";

export function turnoverRate(sold: number, averageStock: number): number | null {
  if (averageStock === 0) {
    return null;
  }
  return sold / averageStock;
}

export function slowMovers(items: StockItem[], soldBySku: Map<string, number>): StockItem[] {
  const slow: StockItem[] = [];
  for (const item of items) {
    const sold = soldBySku.get(item.sku) ?? 0;
    if (sold > 0) {
      continue;
    }
    slow.push(item);
  }
  return slow;
}

export function totalOnHand(items: StockItem[]): number {
  let onHand = 0;
  for (const item of items) {
    onHand += item.quantity;
  }
  return onHand;
}
