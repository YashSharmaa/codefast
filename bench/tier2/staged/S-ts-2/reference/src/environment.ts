// The Environment holds templates and filters and renders template trees.

import { RenderError } from "./errors.ts";
import { builtinFilters, type Filter } from "./filters.ts";
import { Macro, callValue, namespaceOf, type Scope } from "./macros.ts";
import { parseTemplate, type Expr, type Node } from "./parser.ts";
import { Safe, chars, escapeHtml, isPlainObject, lookupSegment, toText, truthy, unwrap } from "./values.ts";

export type EnvironmentOptions = {
  templates?: Record<string, string>;
  loader?: (name: string) => string | undefined;
  strict?: boolean;
  autoescape?: boolean;
  globals?: Record<string, unknown>;
};

type RenderState = {
  data: Record<string, unknown>;
  scopes: Scope[];
  active: string[];
  template: string | null;
  signal?: "break" | "continue";
};

type Loop = { index: number; index0: number; revindex: number; revindex0: number; first: boolean; last: boolean; length: number; depth: number; previtem: unknown; nextitem: unknown };

export class Environment {
  private templates = new Map<string, Node[]>();
  private filters = new Map<string, Filter>(Object.entries(builtinFilters));
  private loader: ((name: string) => string | undefined) | undefined;
  private strict: boolean;
  private autoescape: boolean;
  private globals = new Map<string, unknown>([["range", range]]);

  constructor(options: EnvironmentOptions = {}) {
    this.loader = options.loader;
    this.strict = options.strict ?? false;
    this.autoescape = options.autoescape ?? true;
    for (const [name, value] of Object.entries(options.globals ?? {})) this.addGlobal(name, value);
    for (const [name, source] of Object.entries(options.templates ?? {})) this.addTemplate(name, source);
  }

  addTemplate(name: string, source: string): void {
    this.templates.set(name, parseTemplate(source, name));
  }

  addFilter(name: string, fn: Filter): void {
    this.filters.set(name, fn);
  }

  addGlobal(name: string, value: unknown): void {
    this.globals.set(name, value);
  }

  render(name: string, data: Record<string, unknown> = {}): string {
    const state: RenderState = { data, scopes: [], active: [], template: null };
    return this.renderNamed(name, state);
  }

  renderString(source: string, data: Record<string, unknown> = {}): string {
    const nodes = parseTemplate(source, null);
    return this.renderNodes(nodes, { data, scopes: [new Map()], active: [], template: null });
  }

  private template(name: string): Node[] {
    let nodes = this.templates.get(name);
    if (nodes === undefined && this.loader) {
      const source = this.loader(name);
      if (source !== undefined) {
        nodes = parseTemplate(source, name);
        this.templates.set(name, nodes);
      }
    }
    if (nodes === undefined) throw new RenderError(`unknown template ${JSON.stringify(name)}`);
    return nodes;
  }

  private renderNamed(name: string, state: RenderState, scope: Scope = new Map()): string {
    if (state.active.includes(name)) {
      throw new RenderError(`include cycle: ${[...state.active, name].join(" -> ")}`);
    }
    const nodes = this.template(name);
    const caller = state.template;
    state.active.push(name);
    state.scopes.push(scope);
    state.template = name;
    try {
      return this.renderNodes(nodes, state);
    } finally {
      state.template = caller;
      state.scopes.pop();
      state.active.pop();
    }
  }

  private renderNodes(nodes: Node[], state: RenderState): string {
    let out = "";
    for (const node of nodes) {
      out += this.renderNode(node, state);
      if (state.signal) break;
    }
    return out;
  }

  // Runs fn and gives a RenderError without a line the given line and the current template.
  private at<T>(line: number, state: RenderState, fn: () => T): T {
    try {
      return fn();
    } catch (err) {
      if (err instanceof RenderError && err.line === null) {
        err.line = line;
        err.template = state.template;
      }
      throw err;
    }
  }

  private renderNode(node: Node, state: RenderState): string {
    if (node.type === "if") {
      for (const branch of node.branches) {
        if (truthy(this.at(branch.line, state, () => this.evaluate(branch.cond, state)))) return this.renderNodes(branch.body, state);
      }
      return node.otherwise ? this.renderNodes(node.otherwise, state) : "";
    }
    if (node.type === "for" || node.type === "callblock") return this.at(node.line, state, () => this.renderBlock(node, state));
    return "line" in node ? this.at(node.line, state, () => this.renderTag(node, state)) : this.renderTag(node, state);
  }

  private renderBlock(node: Extract<Node, { type: "for" | "callblock" }>, state: RenderState): string {
    if (node.type === "for") return this.renderFor(node, state);
    const callee = unwrap(this.evaluate(node.call.callee, state));
    if (!(callee instanceof Macro)) throw new RenderError("a call block needs a macro");
    const [args, kwargs] = this.arguments(node.call, state);
    return String(callee.call(args, kwargs, () => this.renderNodes(node.body, state)).value);
  }

  private renderTag(node: Exclude<Node, { type: "if" | "for" | "callblock" }>, state: RenderState): string {
    switch (node.type) {
      case "text":
        return node.text;
      case "output": {
        const value = this.evaluate(node.expr, state);
        if (value === undefined && this.strict) {
          throw new RenderError("undefined value in strict mode");
        }
        const text = toText(value);
        return this.autoescape && !(value instanceof Safe) ? escapeHtml(text) : text;
      }
      case "include":
        return this.renderNamed(node.name, state);
      case "set":
        state.scopes[state.scopes.length - 1].set(node.name, this.evaluate(node.expr, state));
        return "";
      case "macro":
        state.scopes[state.scopes.length - 1].set(node.name, this.defineMacro(node, state));
        return "";
      case "break":
      case "continue":
        state.signal = node.type;
        return "";
      case "import":
        state.scopes[state.scopes.length - 1].set(node.alias, this.importMacros(node.name, state));
        return "";
      case "fromimport": {
        const ns = this.importMacros(node.name, state);
        for (const [name, alias] of node.names) {
          if (!Object.hasOwn(ns, name)) throw new RenderError(`template ${JSON.stringify(node.name)} has no macro ${name}`);
          state.scopes[state.scopes.length - 1].set(alias, ns[name]);
        }
        return "";
      }
    }
  }

  private renderFor(node: Extract<Node, { type: "for" }>, state: RenderState): string {
    const value = unwrap(this.evaluate(node.iterable, state));
    if (value === undefined && this.strict) {
      throw new RenderError("loop over an undefined value in strict mode");
    }
    let items: unknown[][];
    if (value === undefined || value === null) {
      items = [];
    } else if (Array.isArray(value) || typeof value === "string") {
      const elements: unknown[] = typeof value === "string" ? chars(value) : value;
      items = node.names.length === 1 ? elements.map((v) => [v]) : elements.map((v, i) => [i, v]);
    } else if (isPlainObject(value)) {
      items = node.names.length === 1 ? Object.keys(value).map((k) => [k]) : Object.entries(value);
    } else {
      throw new RenderError(`cannot loop over this value with ${node.names.length} variable(s)`);
    }
    if (node.filter) items = items.filter((values) => this.keep(node, values, state));
    if (items.length === 0) return node.otherwise ? this.renderNodes(node.otherwise, state) : "";
    const outer = this.lookup("loop", state);
    const depth = isLoop(outer) ? outer.depth + 1 : 1;
    const n = items.length;
    let out = "";
    for (let i = 0; i < n; i++) {
      const scope: Scope = new Map();
      node.names.forEach((name, j) => scope.set(name, items[i][j]));
      const at = (k: number) => (k >= 0 && k < n ? items[k][node.names.length - 1] : undefined);
      const loop: Loop = { index: i + 1, index0: i, revindex: n - i, revindex0: n - i - 1, first: i === 0, last: i === n - 1, length: n, depth, previtem: at(i - 1), nextitem: at(i + 1) };
      loops.add(loop);
      scope.set("loop", loop);
      state.scopes.push(scope);
      try {
        out += this.renderNodes(node.body, state);
      } finally {
        state.scopes.pop();
      }
      const signal = state.signal;
      state.signal = undefined;
      if (signal === "break") break;
    }
    return out;
  }

  private importMacros(name: string, state: RenderState): Record<string, Macro> {
    const scope: Scope = new Map();
    this.renderNamed(name, { data: {}, scopes: [], active: state.active, template: state.template }, scope);
    return namespaceOf(scope);
  }

  private defineMacro(node: Extract<Node, { type: "macro" }>, state: RenderState): Macro {
    const outer = state.scopes.slice();
    const template = state.template;
    const inner = (scope: Scope): RenderState => ({ data: state.data, scopes: [...outer, scope], active: state.active, template });
    return new Macro(
      node.name,
      node.params,
      node.defaults.map((d) => d !== null),
      (scope) => this.renderNodes(node.body, inner(scope)),
      (i, scope) => this.evaluate(node.defaults[i]!, inner(scope)),
    );
  }

  private arguments(call: Extract<Expr, { type: "call" }>, state: RenderState): [unknown[], Array<[string, unknown]>] {
    const args = call.args.map((a) => this.evaluate(a, state));
    const kwargs = call.kwargs.map(([k, v]): [string, unknown] => [k, this.evaluate(v, state)]);
    return [args, kwargs];
  }

  private keep(node: Extract<Node, { type: "for" }>, values: unknown[], state: RenderState): boolean {
    const scope: Scope = new Map();
    node.names.forEach((name, j) => scope.set(name, values[j]));
    state.scopes.push(scope);
    try {
      return truthy(this.evaluate(node.filter!, state));
    } finally {
      state.scopes.pop();
    }
  }

  private lookup(name: string, state: RenderState): unknown {
    for (let i = state.scopes.length - 1; i >= 0; i--) {
      if (state.scopes[i].has(name)) return state.scopes[i].get(name);
    }
    if (Object.hasOwn(state.data, name)) return state.data[name];
    return this.globals.get(name);
  }

  private evaluate(expr: Expr, state: RenderState): unknown {
    switch (expr.type) {
      case "literal":
        return expr.value;
      case "path": {
        let value = this.lookup(expr.name, state);
        for (const segment of expr.segments) value = lookupSegment(value, segment);
        return value;
      }
      case "not":
        return !truthy(this.evaluate(expr.operand, state));
      case "and":
        return truthy(this.evaluate(expr.left, state)) && truthy(this.evaluate(expr.right, state));
      case "or":
        return truthy(this.evaluate(expr.left, state)) || truthy(this.evaluate(expr.right, state));
      case "compare":
        return compare(expr.op, unwrap(this.evaluate(expr.left, state)), unwrap(this.evaluate(expr.right, state)));
      case "arith":
        return arithmetic(expr.op, unwrap(this.evaluate(expr.left, state)), unwrap(this.evaluate(expr.right, state)));
      case "neg": {
        const value = unwrap(this.evaluate(expr.operand, state));
        if (typeof value !== "number") throw new RenderError(`cannot negate ${typeof value}`);
        return -value;
      }
      case "list":
        return expr.items.map((item) => this.evaluate(item, state));
      case "object": {
        const out: Record<string, unknown> = {};
        for (const [key, value] of expr.entries) out[key] = this.evaluate(value, state);
        return out;
      }
      case "attr":
        return lookupSegment(this.evaluate(expr.target, state), expr.segment);
      case "call": {
        const callee = this.evaluate(expr.callee, state);
        const [args, kwargs] = this.arguments(expr, state);
        return callValue(callee, args, kwargs);
      }
      case "index":
        return lookupKey(unwrap(this.evaluate(expr.target, state)), unwrap(this.evaluate(expr.key, state)));
      case "filter": {
        const fn = this.filters.get(expr.name);
        if (!fn) throw new RenderError(`unknown filter ${expr.name}`);
        const input = unwrap(this.evaluate(expr.input, state));
        const args = expr.args.map((a) => unwrap(this.evaluate(a, state)));
        return fn(input, ...args);
      }
    }
  }
}

const loops = new WeakSet<object>();

function isLoop(value: unknown): value is Loop {
  return typeof value === "object" && value !== null && loops.has(value);
}

function range(...args: unknown[]): number[] {
  if (args.length < 1 || args.length > 3 || !args.every((a) => typeof a === "number" && Number.isInteger(a))) {
    throw new RenderError("range needs one to three integers");
  }
  const [start, stop, step] = args.length === 1 ? [0, args[0] as number, 1] : [args[0] as number, args[1] as number, (args[2] ?? 1) as number];
  if (step === 0) throw new RenderError("range step must not be 0");
  const out: number[] = [];
  for (let i = start; step > 0 ? i < stop : i > stop; i += step) out.push(i);
  return out;
}

function lookupKey(value: unknown, key: unknown): unknown {
  if (Array.isArray(value)) return typeof key === "number" && Number.isInteger(key) && key >= 0 ? value[key] : undefined;
  if (isPlainObject(value) && typeof key === "string" && Object.hasOwn(value, key)) return value[key];
  return undefined;
}

function arithmetic(op: string, a: unknown, b: unknown): unknown {
  if (op === "~") return toText(a) + toText(b);
  if (op === "+") {
    if (typeof a === "string" && typeof b === "string") return a + b;
    if (Array.isArray(a) && Array.isArray(b)) return [...a, ...b];
  }
  if (typeof a !== "number" || typeof b !== "number") {
    throw new RenderError(`cannot apply ${op} to ${typeof a} and ${typeof b}`);
  }
  if ((op === "/" || op === "//" || op === "%") && b === 0) throw new RenderError("division by zero");
  if (op === "+") return a + b;
  if (op === "-") return a - b;
  if (op === "*") return a * b;
  if (op === "/") return a / b;
  if (op === "//") return Math.floor(a / b);
  return a - b * Math.floor(a / b);
}

function compare(op: string, a: unknown, b: unknown): boolean {
  if (op === "==") return a === b;
  if (op === "!=") return a !== b;
  if (op === "in") {
    if (Array.isArray(b)) return b.includes(a);
    if (typeof b === "string") return typeof a === "string" && b.includes(a);
    if (isPlainObject(b)) return typeof a === "string" && Object.hasOwn(b, a);
    throw new RenderError("the right side of in must be an array, string or object");
  }
  const bothNumbers = typeof a === "number" && typeof b === "number";
  const bothStrings = typeof a === "string" && typeof b === "string";
  if (!bothNumbers && !bothStrings) throw new RenderError(`cannot compare ${typeof a} with ${typeof b}`);
  const x = a as number | string;
  const y = b as number | string;
  if (op === "<") return x < y;
  if (op === "<=") return x <= y;
  if (op === ">") return x > y;
  return x >= y;
}
