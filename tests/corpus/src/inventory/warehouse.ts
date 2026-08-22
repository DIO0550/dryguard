import { StockItem, availableQuantity } from "./stock";

const MAX_CAPACITY = 10000;

export function capacityLeft(items: StockItem[]): number {
  let used = 0;
  for (const item of items) {
    used += item.quantity;
  }
  return Math.max(MAX_CAPACITY - used, 0);
}

export function allocate(item: StockItem, requested: number): number {
  const available = availableQuantity(item);
  return Math.min(available, requested);
}

export function itemsInWarehouse(items: StockItem[], warehouseId: string): StockItem[] {
  const here: StockItem[] = [];
  for (const item of items) {
    if (item.warehouseId !== warehouseId) {
      continue;
    }
    here.push(item);
  }
  return here;
}
