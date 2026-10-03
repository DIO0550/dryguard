import type { ServerCapabilities } from "./server";

export function providesHover(capabilities: ServerCapabilities): boolean {
  return capabilities.hoverProvider === true;
}

export function providesReferences(capabilities: ServerCapabilities): boolean {
  return capabilities.referencesProvider === true;
}

export function directoryOf(path: string): string {
  const parts = path.split("/");
  return parts.slice(0, -1).join("/");
}

export function folderOf(file: string): string {
  const segments = file.split("/");
  return segments.slice(0, -1).join("/");
}
