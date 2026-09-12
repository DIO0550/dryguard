export function firstOf<T>(items: T[]): T | undefined {
  for (const item of items) {
    return item;
  }
  return undefined;
}
