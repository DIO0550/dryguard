export namespace money {
  export type Amount = number;
}

export function scaleQualified(amount: money.Amount, factor: number): money.Amount {
  return amount * factor;
}
