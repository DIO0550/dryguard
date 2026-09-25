export interface Exemption {
  customerId: string;
  region: string;
  expiresAt: Date;
}

export function activeExemptions(exemptions: Exemption[], asOf: Date): Exemption[] {
  const active: Exemption[] = [];
  for (const exemption of exemptions) {
    if (exemption.expiresAt.getTime() <= asOf.getTime()) {
      continue;
    }
    active.push(exemption);
  }
  return active;
}

export function isExempt(exemptions: Exemption[], customerId: string, region: string): boolean {
  for (const exemption of exemptions) {
    if (exemption.customerId === customerId && exemption.region === region) {
      return true;
    }
  }
  return false;
}
