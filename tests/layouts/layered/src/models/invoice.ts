export interface Invoice {
  id: string;
  customerId: string;
  amount: number;
  dueAt: Date;
  paid: boolean;
}
