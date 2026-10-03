export function makeAdder(base: number) {
  return (value: number) => ((base + value) * (base - value)) / 2;
}
