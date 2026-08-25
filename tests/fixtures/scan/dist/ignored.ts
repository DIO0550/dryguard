export function reorderPoint(level: number, buffer: number): number {
  const point = level * (1 + buffer);
  return Math.max(point, 0);
}
