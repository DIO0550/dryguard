const RATES_BY_REGION: Record<string, number> = {
  eu: 0.2,
  jp: 0.1,
  us: 0.07,
};

const DEFAULT_RATE = 0.1;

export function rateFor(region: string): number {
  const rate = RATES_BY_REGION[region];
  if (rate === undefined) {
    return DEFAULT_RATE;
  }
  return rate;
}

export function taxFor(amount: number, region: string): number {
  const rate = rateFor(region);
  return Math.round(amount * rate);
}
