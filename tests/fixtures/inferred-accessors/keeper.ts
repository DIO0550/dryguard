import { makeShipment } from "./shipment";

export class Keeper {
  get value() {
    const made = makeShipment();
    return made;
  }
}
