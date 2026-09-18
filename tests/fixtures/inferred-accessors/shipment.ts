export interface Shipment {
  crates: number;
}

export function makeShipment(): Shipment {
  return { crates: 0 };
}
