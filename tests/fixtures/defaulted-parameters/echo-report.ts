import { Receipt } from "./shared";
import { makeReceipt } from "./report";

export function echo(received = makeReceipt()): Receipt {
  return received;
}
