export function applyDiscount(amount: number, rate: number): number {
  const discounted = amount * (1 - rate);
  return Math.max(discounted, 0);
}
