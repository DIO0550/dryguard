import { reorderAmount } from "./reorder";

export type Stock = {
  quantity: number;
};

export function stockToOrder(stock: Stock): number {
  return reorderAmount(stock);
}
