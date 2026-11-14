// Macros and calls: binding call arguments to parameters, calling macros and functions.

import { RenderError } from "./errors.ts";
import { Safe, unwrap } from "./values.ts";

export type Scope = Map<string, unknown>;

// Renders the macro body with the given parameter scope and returns the text.
export type MacroBody = (scope: Scope) => string;

// Evaluates the default of parameter i in the scope that holds the parameters bound so far.
export type DefaultValue = (index: number, scope: Scope) => unknown;

export class Macro {
  name: string;
  params: string[];
  hasDefault: boolean[];
  private body: MacroBody;
  private defaultValue: DefaultValue;

  constructor(name: string, params: string[], hasDefault: boolean[], body: MacroBody, defaultValue: DefaultValue) {
    this.name = name;
    this.params = params;
    this.hasDefault = hasDefault;
    this.body = body;
    this.defaultValue = defaultValue;
  }

  call(args: unknown[], kwargs: Array<[string, unknown]>, caller?: () => string): Safe {
    if (args.length > this.params.length) {
      throw new RenderError(`macro ${this.name} takes at most ${this.params.length} argument(s), got ${args.length}`);
    }
    const given = new Map<string, unknown>();
    args.forEach((value, i) => given.set(this.params[i], value));
    for (const [key, value] of kwargs) {
      if (!this.params.includes(key)) throw new RenderError(`macro ${this.name} has no parameter ${key}`);
      if (given.has(key)) throw new RenderError(`macro ${this.name} got parameter ${key} twice`);
      given.set(key, value);
    }
    const scope: Scope = new Map();
    this.params.forEach((param, i) => {
      if (given.has(param)) scope.set(param, given.get(param));
      else scope.set(param, this.hasDefault[i] ? this.defaultValue(i, scope) : undefined);
    });
    if (caller) scope.set("caller", new Caller(caller));
    return new Safe(this.body(scope));
  }
}

// The value of `caller` inside a macro called from a call block.
export class Caller {
  private render: () => string;

  constructor(render: () => string) {
    this.render = render;
  }

  call(args: unknown[], kwargs: Array<[string, unknown]>): Safe {
    if (args.length > 0 || kwargs.length > 0) throw new RenderError("caller takes no arguments");
    return new Safe(this.render());
  }
}

export function callValue(callee: unknown, args: unknown[], kwargs: Array<[string, unknown]>): unknown {
  callee = unwrap(callee);
  if (callee instanceof Macro || callee instanceof Caller) return callee.call(args, kwargs);
  if (typeof callee === "function") {
    if (kwargs.length > 0) throw new RenderError("functions take no keyword arguments");
    return callee(...args.map(unwrap));
  }
  throw new RenderError(`cannot call a value of type ${callee === null ? "null" : typeof callee}`);
}

// The namespace of an imported template: its top-level macros by name, in definition order.
export function namespaceOf(scope: Scope): Record<string, Macro> {
  const ns: Record<string, Macro> = {};
  for (const [name, value] of scope) if (value instanceof Macro) ns[name] = value;
  return ns;
}
