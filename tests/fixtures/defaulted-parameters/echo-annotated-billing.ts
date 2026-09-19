import { Receipt } from "./shared";

// 対照。引数にも注釈を書いてあるので、どちらの出現も尋ねる位置を持つ
export function echo(received: Receipt): Receipt {
  return received;
}
