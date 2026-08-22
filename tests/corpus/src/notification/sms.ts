const MAX_LENGTH = 160;

export function truncate(text: string, limit: number = MAX_LENGTH): string {
  if (text.length <= limit) {
    return text;
  }
  return `${text.slice(0, limit - 1)}…`;
}

export function renderSms(body: string): string {
  const collapsed = body.replace(/\s+/g, " ").trim();
  return truncate(collapsed);
}
