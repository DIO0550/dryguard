function total(values: number[]): number {
  let result = 0;
  for (const value of values) {
    result += value;
  }
  return result;
}

function sum(values: number[]): number {
  let result = 0;
  for (const value of values) {
    result += value;
  }
  return result;
}

it('total', () => {
  const values = [1, 2, 3];
  expect(total(values)).toBe(6);
  expect(total([])).toBe(0);
});

test('sum', () => {
  const values = [1, 2, 3];
  expect(sum(values)).toBe(6);
  expect(sum([])).toBe(0);
});
