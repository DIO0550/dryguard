import { Movement, netMovement, safetyStock } from "../utils/helpers";
import { formatDate, skuLabel } from "../utils/format";

export interface Location {
  warehouseId: string;
  sku: string;
  movements: Movement[];
  opening: number;
}

export function onHandAt(location: Location): number {
  return Math.max(location.opening + netMovement(location.movements), 0);
}

export function needsReplenishment(location: Location, dailyDemand: number, leadTimeDays: number): boolean {
  return onHandAt(location) < safetyStock(dailyDemand, leadTimeDays);
}

export function binLabel(location: Location, countedAt: Date): string {
  const label = skuLabel(location.sku, location.warehouseId);
  return `${label} ${onHandAt(location)} ${formatDate(countedAt)}`;
}
