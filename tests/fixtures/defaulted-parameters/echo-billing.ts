import { Receipt } from "./shared";
import { makeReceipt } from "./billing";

// 既定値つきの引数は注釈を省ける。hover は `function echo(received?: Receipt): Receipt` と
// 綴るが、引数の `Receipt` は `billing.ts` のほうで、**戻り値の `Receipt` とは別の出現**
export function echo(received = makeReceipt()): Receipt {
  return received;
}
