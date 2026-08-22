const CENTS_IN_UNIT = 100;

export function roundToCents(amount: number): number {
  return Math.round(amount * CENTS_IN_UNIT) / CENTS_IN_UNIT;
}

export function formatCurrency(amount: number, currency: string): string {
  const rounded = roundToCents(amount);
  return `${currency} ${rounded.toFixed(2)}`;
}

export function parseAmount(text: string): number | null {
  const cleaned = text.replace(/[, ]/g, "");
  const value = Number(cleaned);
  if (!Number.isFinite(value)) {
    return null;
  }
  return roundToCents(value);
}
