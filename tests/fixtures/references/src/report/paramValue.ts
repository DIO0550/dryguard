export function alsoSame(x: string): typeof x {
  const kept = x;
  const seen = kept;
  void seen;
  return kept;
}
