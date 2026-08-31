interface Local<T> {
  readonly amount: T;
}

export type Charged = Local<string>;

export function labelCharged(value: Charged): string {
  return value.amount;
}
