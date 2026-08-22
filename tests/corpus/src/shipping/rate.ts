import { roundToCents } from "../shared/money";

const BASE_RATE = 5.0;
const PER_KG = 1.25;
const REMOTE_SURCHARGE = 0.15;

export function rateFor(weightKg: number): number {
  const rate = BASE_RATE + weightKg * PER_KG;
  return roundToCents(rate);
}

export function surcharge(rate: number, remote: boolean): number {
  if (!remote) {
    return 0;
  }
  return roundToCents(rate * REMOTE_SURCHARGE);
}
