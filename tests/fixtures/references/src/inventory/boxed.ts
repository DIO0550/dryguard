interface Local {
  readonly label: string;
}

export type Wrapped = Local;

export function labelWrapped(value: Wrapped): string {
  return value.label;
}
