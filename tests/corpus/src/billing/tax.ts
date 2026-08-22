import { roundToCents } from "../shared/money";

const DEFAULT_RATE = 0.08;

export function taxFor(amount: number, rate: number = DEFAULT_RATE): number {
  return roundToCents(amount * rate);
}

export function netOf(gross: number, rate: number = DEFAULT_RATE): number {
  return roundToCents(gross / (1 + rate));
}
