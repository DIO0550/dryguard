import type { Shape } from "../shared/shape";

export class Crated<T extends Shape> {
  readonly held: T;

  constructor(value: T) {
    this.held = value;
  }
}
