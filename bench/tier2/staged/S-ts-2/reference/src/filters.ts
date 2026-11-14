// Built-in filters. Each receives the plain value followed by its arguments.

import { RenderError } from "./errors.ts";
import { Safe, chars, isPlainObject, toText } from "./values.ts";

export type Filter = (value: unknown, ...args: unknown[]) => unknown;

function arity(name: string, args: unknown[], max: number): void {
  if (args.length > max) throw new RenderError(`filter ${name} takes at most ${max} argument(s)`);
}

function needString(name: string, value: unknown): string {
  if (typeof value !== "string") throw new RenderError(`filter ${name} needs a string`);
  return value;
}

function needInteger(name: string, value: unknown): number {
  if (typeof value !== "number" || !Number.isInteger(value) || value < 0) {
    throw new RenderError(`filter ${name} needs a non-negative integer argument`);
  }
  return value;
}

export const builtinFilters: Record<string, Filter> = {
  upper: (value, ...args) => (arity("upper", args, 0), toText(value).toUpperCase()),
  lower: (value, ...args) => (arity("lower", args, 0), toText(value).toLowerCase()),
  trim: (value, ...args) => (arity("trim", args, 0), toText(value).trim()),
  length: (value, ...args) => {
    arity("length", args, 0);
    if (value === null || value === undefined) return 0;
    if (typeof value === "string") return chars(value).length;
    if (Array.isArray(value)) return value.length;
    if (isPlainObject(value)) return Object.keys(value).length;
    throw new RenderError("filter length needs a string, array or object");
  },
  default: (value, ...args) => {
    arity("default", args, 1);
    return value === undefined || value === null ? args[0] : value;
  },
  join: (value, ...args) => {
    arity("join", args, 1);
    if (!Array.isArray(value)) throw new RenderError("filter join needs an array");
    const sep = args.length > 0 ? needString("join", args[0]) : ",";
    return value.map((v) => toText(v)).join(sep);
  },
  first: (value, ...args) => {
    arity("first", args, 0);
    if (typeof value === "string") return chars(value)[0];
    if (Array.isArray(value)) return value.length > 0 ? value[0] : undefined;
    throw new RenderError("filter first needs a string or array");
  },
  last: (value, ...args) => {
    arity("last", args, 0);
    if (typeof value === "string") return chars(value).at(-1);
    if (Array.isArray(value)) return value.length > 0 ? value[value.length - 1] : undefined;
    throw new RenderError("filter last needs a string or array");
  },
  truncate: (value, ...args) => {
    arity("truncate", args, 1);
    const text = chars(needString("truncate", value));
    const n = needInteger("truncate", args[0]);
    return text.length <= n ? text.join("") : text.slice(0, n).join("") + "...";
  },
  replace: (value, ...args) => {
    arity("replace", args, 2);
    const text = needString("replace", value);
    const from = needString("replace", args[0]);
    const to = needString("replace", args[1]);
    return text.split(from).join(to);
  },
  round: (value, ...args) => {
    arity("round", args, 1);
    if (typeof value !== "number") throw new RenderError("filter round needs a number");
    const d = args.length > 0 ? needInteger("round", args[0]) : 0;
    return Math.round(value * 10 ** d) / 10 ** d;
  },
  sort: (value, ...args) => {
    arity("sort", args, 0);
    if (!Array.isArray(value)) throw new RenderError("filter sort needs an array");
    if (value.every((v) => typeof v === "number")) return [...value].sort((a, b) => a - b);
    if (value.every((v) => typeof v === "string")) return [...value].sort();
    throw new RenderError("filter sort needs only numbers or only strings");
  },
  raw: (value, ...args) => (arity("raw", args, 0), new Safe(value)),
};
