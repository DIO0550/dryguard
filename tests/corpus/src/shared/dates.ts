const MS_IN_DAY = 24 * 60 * 60 * 1000;

export function pad(value: number): string {
  if (value < 10) {
    return `0${value}`;
  }
  return String(value);
}

export function formatDate(value: Date): string {
  const month = pad(value.getMonth() + 1);
  const day = pad(value.getDate());
  return `${value.getFullYear()}-${month}-${day}`;
}

export function daysBetween(from: Date, to: Date): number {
  const diff = to.getTime() - from.getTime();
  return Math.floor(diff / MS_IN_DAY);
}
