const RATE = 0.1;

export function applyDiscount(amount: number): number {
  const discounted = amount * (1 - RATE);
  return Math.max(discounted, 0);
}
