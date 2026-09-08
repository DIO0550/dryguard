export function parse(value: string): string;
export function parse(value: number): number;
export function parse(value: unknown): unknown {
  return value;
}
