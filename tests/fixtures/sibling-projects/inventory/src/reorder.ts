const THRESHOLD = 0.2;

export function reorderAmount(quantity: number): number {
  const shortage = quantity * (1 - THRESHOLD);
  return Math.max(shortage, 0);
}
