import { Product } from "../models/product";
import { ProductRepository } from "../repositories/productRepository";

export class ProductService {
  constructor(private readonly repository: ProductRepository) {}

  totalOnHand(): number {
    let total = 0;
    for (const product of this.repository.findActive()) {
      total += product.onHand;
    }
    return total;
  }

  belowReorderLevel(): Product[] {
    const low: Product[] = [];
    for (const product of this.repository.findActive()) {
      if (product.onHand > product.reorderLevel) {
        continue;
      }
      low.push(product);
    }
    return low;
  }

  archive(sku: string): boolean {
    const product = this.repository.findBySku(sku);
    if (product === null) {
      return false;
    }
    this.repository.save({ ...product, archived: true });
    return true;
  }
}
