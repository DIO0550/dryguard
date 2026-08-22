const MAX_ATTEMPTS = 5;

export interface Message {
  id: string;
  body: string;
  attempts: number;
}

export function enqueue(queue: Message[], message: Message): Message[] {
  return [...queue, message];
}

export function shouldRetry(message: Message): boolean {
  return message.attempts < MAX_ATTEMPTS;
}

export function retryable(queue: Message[]): Message[] {
  const pending: Message[] = [];
  for (const message of queue) {
    if (!shouldRetry(message)) {
      continue;
    }
    pending.push(message);
  }
  return pending;
}

export function drain(queue: Message[], limit: number): Message[] {
  const batch: Message[] = [];
  for (const message of queue) {
    if (batch.length >= limit) {
      break;
    }
    batch.push(message);
  }
  return batch;
}
