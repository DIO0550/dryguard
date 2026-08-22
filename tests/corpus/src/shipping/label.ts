import { formatDate } from "../shared/dates";

const TRACKING_BASE = "https://track.example.com";

export interface Shipment {
  id: string;
  carrier: string;
  trackingNumber: string;
  shippedAt: Date;
}

export function trackingUrl(shipment: Shipment): string {
  return `${TRACKING_BASE}/${shipment.carrier}/${shipment.trackingNumber}`;
}

export function buildLabel(shipment: Shipment): string {
  const shipped = formatDate(shipment.shippedAt);
  return `${shipment.carrier} ${shipment.trackingNumber} (${shipped})`;
}
