export interface TrackingEvent {
  status: string;
  occurredAt: Date;
  location: string;
}

export function latestStatus(events: TrackingEvent[]): string | null {
  if (events.length === 0) {
    return null;
  }
  let latest = events[0];
  for (const event of events) {
    if (event.occurredAt.getTime() > latest.occurredAt.getTime()) {
      latest = event;
    }
  }
  return latest.status;
}

export function eventsAfter(events: TrackingEvent[], since: Date): TrackingEvent[] {
  const recent: TrackingEvent[] = [];
  for (const event of events) {
    if (event.occurredAt.getTime() <= since.getTime()) {
      continue;
    }
    recent.push(event);
  }
  return recent;
}
