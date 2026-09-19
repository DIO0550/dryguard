import { makeReceipt } from "./receipt";

export class Holder {
  get value() {
    const made = makeReceipt();
    return made;
  }
}
