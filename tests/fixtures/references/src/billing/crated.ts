import type { Shape } from "../shared/shape";

export class Crated<T extends Shape> {
  readonly value: T;

  constructor(value: T) {
    this.value = value;
  }
}
