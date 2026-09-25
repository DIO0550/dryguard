import { Product } from "../models/product";

export class ProductRepository {
  private readonly products = new Map<string, Product>();

  save(product: Product): void {
    this.products.set(product.sku, product);
  }

  findBySku(sku: string): Product | null {
    const product = this.products.get(sku);
    if (product === undefined) {
      return null;
    }
    return product;
  }

  findActive(): Product[] {
    const found: Product[] = [];
    for (const product of this.products.values()) {
      if (product.archived) {
        continue;
      }
      found.push(product);
    }
    return found;
  }
}
