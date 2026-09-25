export interface Product {
  sku: string;
  name: string;
  onHand: number;
  reorderLevel: number;
  archived: boolean;
}
