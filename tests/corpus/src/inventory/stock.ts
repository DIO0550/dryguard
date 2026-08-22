export interface StockItem {
  sku: string;
  quantity: number;
  reserved: number;
  warehouseId: string;
}

export function availableQuantity(item: StockItem): number {
  const free = item.quantity - item.reserved;
  return Math.max(free, 0);
}
