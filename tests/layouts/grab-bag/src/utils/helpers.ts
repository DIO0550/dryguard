export interface Charge {
  amount: number;
  chargedAt: Date;
  disputed: boolean;
}

export interface Movement {
  sku: string;
  quantity: number;
  movedAt: Date;
  cancelled: boolean;
}

const LATE_FEE_RATE = 0.015;

export function lateFee(balance: number, monthsLate: number): number {
  if (monthsLate <= 0) {
    return 0;
  }
  return Math.round(balance * LATE_FEE_RATE * monthsLate);
}

export function settledTotal(charges: Charge[]): number {
  let total = 0;
  for (const charge of charges) {
    if (charge.disputed) {
      continue;
    }
    total += charge.amount;
  }
  return total;
}

export function netMovement(movements: Movement[]): number {
  let total = 0;
  for (const movement of movements) {
    if (movement.cancelled) {
      continue;
    }
    total += movement.quantity;
  }
  return total;
}

export function safetyStock(dailyDemand: number, leadTimeDays: number): number {
  if (leadTimeDays <= 0) {
    return 0;
  }
  return Math.ceil(dailyDemand * leadTimeDays * 0.5);
}

export function chunk<T>(items: T[], size: number): T[][] {
  const chunks: T[][] = [];
  for (let i = 0; i < items.length; i += size) {
    chunks.push(items.slice(i, i + size));
  }
  return chunks;
}
