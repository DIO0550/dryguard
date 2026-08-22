import { StockItem, availableQuantity } from "./stock";

const REORDER_THRESHOLD = 5;

export function reorderAmount(item: StockItem): number {
  const shortage = REORDER_THRESHOLD - availableQuantity(item);
  return Math.max(shortage, 0);
}

export function reorderPlan(items: StockItem[]): Map<string, number> {
  const plan = new Map<string, number>();
  for (const item of items) {
    const amount = reorderAmount(item);
    if (amount === 0) {
      continue;
    }
    plan.set(item.sku, amount);
  }
  return plan;
}
