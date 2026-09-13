const localValue = { rowCount: 0 };

export function currentValue(): typeof localValue {
  return localValue;
}
