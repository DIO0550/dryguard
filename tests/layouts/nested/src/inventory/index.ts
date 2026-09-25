import { StockItem, activeItems } from "./stock/item";
import { CountResult, discrepancies, itemsAt, totalAvailable } from "./stock/count";
import { Supplier, activeSuppliers, fastestSupplier } from "./purchasing/supplier";
import { reorderQuantity } from "./purchasing/reorder";
import { inventoryValue, writeOffs } from "./valuation/cost";

export function warehouseAvailability(items: StockItem[], warehouseId: string): number {
  return totalAvailable(activeItems(itemsAt(items, warehouseId)));
}

export function reorderPlan(
  items: StockItem[],
  dailyDemand: number,
  suppliers: Supplier[],
): Map<string, number> {
  const plan = new Map<string, number>();
  const supplier = fastestSupplier(activeSuppliers(suppliers));
  if (supplier === null) {
    return plan;
  }
  for (const item of activeItems(items)) {
    plan.set(item.sku, reorderQuantity(item, dailyDemand, supplier));
  }
  return plan;
}

export function auditFindings(items: StockItem[], counts: CountResult[]): number {
  return discrepancies(items, counts).length;
}

export function writeOffValue(items: StockItem[]): number {
  return inventoryValue(writeOffs(items));
}
