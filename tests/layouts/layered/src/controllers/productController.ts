import { ProductService } from "../services/productService";
import { Response } from "./invoiceController";

export class ProductController {
  constructor(private readonly service: ProductService) {}

  getStockLevel(): Response {
    const onHand = this.service.totalOnHand();
    return { status: 200, body: { onHand } };
  }

  getLowStock(): Response {
    const low = this.service.belowReorderLevel();
    return { status: 200, body: low };
  }

  deleteProduct(sku: string): Response {
    if (!this.service.archive(sku)) {
      return { status: 404, body: { error: `product ${sku} not found` } };
    }
    return { status: 204, body: null };
  }
}
