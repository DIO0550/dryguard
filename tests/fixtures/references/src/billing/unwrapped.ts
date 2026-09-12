export function unwrapCharged<T>(value: T): T extends Promise<infer U> ? U : never {
  return value as never;
}
