import { Rate } from "../shared/rate";

export function scaleByRate(value: Rate, factor: Rate): Rate {
  return value * factor;
}

export function scaleByNumber(value: number, factor: number): number {
  return value * factor;
}
