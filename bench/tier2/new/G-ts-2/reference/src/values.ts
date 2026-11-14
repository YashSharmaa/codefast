// Value helpers shared by filters and the renderer: safe strings, text conversion, truthiness.

import { RenderError } from "./errors.ts";

export class Safe {
  value: unknown;

  constructor(value: unknown) {
    this.value = value;
  }
}

export function unwrap(value: unknown): unknown {
  return value instanceof Safe ? value.value : value;
}

export function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value) && !(value instanceof Safe);
}

export function toText(value: unknown): string {
  value = unwrap(value);
  if (value === null || value === undefined) return "";
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  if (Array.isArray(value)) return value.map((v) => toText(v)).join(",");
  throw new RenderError(`cannot output a value of type ${typeof value}`);
}

export function truthy(value: unknown): boolean {
  value = unwrap(value);
  if (Array.isArray(value)) return value.length > 0;
  if (isPlainObject(value)) return Object.keys(value).length > 0;
  return Boolean(value);
}

const ESCAPES: Record<string, string> = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" };

export function escapeHtml(text: string): string {
  return text.replace(/[&<>"']/g, (ch) => ESCAPES[ch]);
}

export function lookupSegment(value: unknown, segment: string): unknown {
  value = unwrap(value);
  if (Array.isArray(value) && /^\d+$/.test(segment)) return value[Number(segment)];
  if (typeof value === "object" && value !== null && !Array.isArray(value) && Object.hasOwn(value, segment)) {
    return (value as Record<string, unknown>)[segment];
  }
  return undefined;
}
