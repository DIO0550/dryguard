export function nextSequence(current: number): number {
  return ((current + 1) * (current - 1)) / 2;
}
