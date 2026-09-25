export interface Supplier {
  id: string;
  leadTimeDays: number;
  minimumOrder: number;
  active: boolean;
}

export function activeSuppliers(suppliers: Supplier[]): Supplier[] {
  const active: Supplier[] = [];
  for (const supplier of suppliers) {
    if (!supplier.active) {
      continue;
    }
    active.push(supplier);
  }
  return active;
}

export function fastestSupplier(suppliers: Supplier[]): Supplier | null {
  let fastest: Supplier | null = null;
  for (const supplier of suppliers) {
    if (fastest === null || supplier.leadTimeDays < fastest.leadTimeDays) {
      fastest = supplier;
    }
  }
  return fastest;
}
