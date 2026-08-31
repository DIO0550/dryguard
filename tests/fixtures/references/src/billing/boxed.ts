interface Local {
  readonly amount: number;
}

export type Boxed = Local;

export function labelBoxed(value: Boxed): string {
  return String(value.amount);
}
