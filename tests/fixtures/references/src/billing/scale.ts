import { Amount, Scaling } from "./money";

export function scaleAmount(amount: Amount, factor: number): Amount {
  return amount * factor;
}

export const halveAmount: Scaling = (amount) => amount / 2;
