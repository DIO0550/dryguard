import { Row } from "./row";

export function summarize(rows: Row[]): string {
  const lines: string[] = [];
  for (const row of rows) {
    lines.push(`${row.label}: ${row.value}`);
  }
  return lines.join("\n");
}
