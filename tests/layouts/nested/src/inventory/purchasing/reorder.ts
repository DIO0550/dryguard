import { StockItem, available } from "../stock/item";
import { Supplier } from "./supplier";

const SAFETY_DAYS = 3;

export function reorderPoint(dailyDemand: number, supplier: Supplier): number {
  return Math.ceil(dailyDemand * (supplier.leadTimeDays + SAFETY_DAYS));
}

export function reorderQuantity(item: StockItem, dailyDemand: number, supplier: Supplier): number {
  const shortfall = reorderPoint(dailyDemand, supplier) - available(item);
  if (shortfall <= 0) {
    return 0;
  }
  return Math.max(shortfall, supplier.minimumOrder);
}
