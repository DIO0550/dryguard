export function headOf<U>(values: U[]): U | undefined {
  for (const value of values) {
    return value;
  }
  return undefined;
}
