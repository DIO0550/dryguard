const localValue = { invoiceId: "" };

export function currentValue(): typeof localValue {
  return localValue;
}
