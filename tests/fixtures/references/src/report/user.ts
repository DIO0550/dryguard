export interface User {
  readonly rowCount: number;
}

export function labelUser(value: User): string {
  return String(value.rowCount);
}
