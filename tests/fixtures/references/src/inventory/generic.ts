interface Local<T> {
  readonly label: T;
  readonly count: number;
}

export type Tagged = Local<string>;

export function labelTagged(value: Tagged): string {
  return value.label;
}
