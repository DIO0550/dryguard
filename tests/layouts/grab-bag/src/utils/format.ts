export function formatDate(date: Date): string {
  return date.toISOString().slice(0, 10);
}

export function invoiceNumber(sequence: number, issuedAt: Date): string {
  const year = issuedAt.getUTCFullYear();
  return `INV-${year}-${String(sequence).padStart(6, "0")}`;
}

export function skuLabel(sku: string, warehouseId: string): string {
  const zone = warehouseId.toUpperCase();
  return `${zone}/${sku.padStart(8, "0")}`;
}
