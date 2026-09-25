export interface StockItem {
  sku: string;
  warehouseId: string;
  onHand: number;
  reserved: number;
  unitCost: number;
  discontinued: boolean;
}

export function available(item: StockItem): number {
  return Math.max(item.onHand - item.reserved, 0);
}

export function activeItems(items: StockItem[]): StockItem[] {
  const active: StockItem[] = [];
  for (const item of items) {
    if (item.discontinued) {
      continue;
    }
    active.push(item);
  }
  return active;
}
