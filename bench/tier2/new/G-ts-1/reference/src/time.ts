// Times are YYYY-MM-DDTHH:MM in UTC, held internally as whole minutes since the epoch.

import { ValidationError } from "./errors.ts";

const TIME_RE = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})$/;

export function parseTime(text: string): number {
  if (typeof text !== "string") {
    throw new ValidationError(`time must be a string, got ${typeof text}`);
  }
  const m = TIME_RE.exec(text);
  if (!m) {
    throw new ValidationError(`bad time ${JSON.stringify(text)}: expected YYYY-MM-DDTHH:MM`);
  }
  const [year, month, day, hour, minute] = m.slice(1).map(Number);
  const ms = Date.UTC(year, month - 1, day, hour, minute);
  if (hour > 23 || minute > 59 || formatTime(ms / 60000) !== text) {
    throw new ValidationError(`bad time ${JSON.stringify(text)}: no such moment`);
  }
  return ms / 60000;
}

function pad(n: number, width = 2): string {
  return String(n).padStart(width, "0");
}

export function formatTime(minutes: number): string {
  const d = new Date(minutes * 60000);
  return (
    `${pad(d.getUTCFullYear(), 4)}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())}` +
    `T${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())}`
  );
}
