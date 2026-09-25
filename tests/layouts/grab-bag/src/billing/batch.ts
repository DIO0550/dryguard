import { chunk } from "../utils/helpers";
import { Account, balanceOf } from "./account";

const BATCH_SIZE = 50;

export function accountsToBill(accounts: Account[]): Account[][] {
  const billable: Account[] = [];
  for (const account of accounts) {
    if (balanceOf(account) === 0) {
      continue;
    }
    billable.push(account);
  }
  return chunk(billable, BATCH_SIZE);
}
