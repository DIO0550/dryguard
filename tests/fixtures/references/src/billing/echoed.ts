import { Receipt } from "../shared/receipt";
import { buildReceipt } from "./inferred";

export function echoReceipt(received: Receipt) {
  void received;
  return buildReceipt(0);
}
