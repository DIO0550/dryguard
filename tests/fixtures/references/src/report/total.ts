export function scaleTotal(total: number, factor: number): number {
  return total * factor;
}

export const halveTotal: (total: number) => number = (total) => total / 2;
