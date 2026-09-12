export class Carton {
  readonly quantity: number;

  constructor(amount: string) {
    this.quantity = amount.length;
  }
}
