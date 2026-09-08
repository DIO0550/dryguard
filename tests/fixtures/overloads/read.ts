export function read(input: string): string;
export function read(input: Date): Date;
export function read(input: unknown): unknown {
  return input;
}
