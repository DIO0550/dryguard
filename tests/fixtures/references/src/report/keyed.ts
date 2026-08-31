export function keyedByProperty(key: PropertyKey): string {
  return String(key);
}

export function keyedByUnion(key: string | number | symbol): string {
  return String(key);
}
