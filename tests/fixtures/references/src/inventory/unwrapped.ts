export function unwrapStocked<A>(value: A): A extends Promise<infer U> ? U : never {
  return value as never;
}
