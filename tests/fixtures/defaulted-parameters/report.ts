export interface Receipt {
  id: string;
  signedBy: string;
}

export function makeReceipt(): Receipt {
  return { id: "report", signedBy: "report" };
}
