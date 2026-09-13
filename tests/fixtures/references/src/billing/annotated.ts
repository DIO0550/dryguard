import { Receipt } from "../shared/receipt";
import { buildLocal } from "./localReceipt";

export function echoReceipt(received: Receipt): Receipt {
  void received;
  return buildLocal();
}
