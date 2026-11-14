// The Environment holds templates and filters and renders template trees.

import { RenderError } from "./errors.ts";
import { builtinFilters, type Filter } from "./filters.ts";
import { parseTemplate, type Expr, type Node } from "./parser.ts";
import { Safe, escapeHtml, isPlainObject, lookupSegment, toText, truthy, unwrap } from "./values.ts";

export type EnvironmentOptions = {
  templates?: Record<string, string>;
  loader?: (name: string) => string | undefined;
  strict?: boolean;
  autoescape?: boolean;
};

type Scope = Map<string, unknown>;

type RenderState = { data: Record<string, unknown>; scopes: Scope[]; active: string[] };

export class Environment {
  private templates = new Map<string, Node[]>();
  private filters = new Map<string, Filter>(Object.entries(builtinFilters));
  private loader: ((name: string) => string | undefined) | undefined;
  private strict: boolean;
  private autoescape: boolean;

  constructor(options: EnvironmentOptions = {}) {
    this.loader = options.loader;
    this.strict = options.strict ?? false;
    this.autoescape = options.autoescape ?? true;
    for (const [name, source] of Object.entries(options.templates ?? {})) this.addTemplate(name, source);
  }

  addTemplate(name: string, source: string): void {
    this.templates.set(name, parseTemplate(source));
  }

  addFilter(name: string, fn: Filter): void {
    this.filters.set(name, fn);
  }

  render(name: string, data: Record<string, unknown> = {}): string {
    const state: RenderState = { data, scopes: [], active: [] };
    return this.renderNamed(name, state);
  }

  renderString(source: string, data: Record<string, unknown> = {}): string {
    const nodes = parseTemplate(source);
    return this.renderNodes(nodes, { data, scopes: [], active: [] });
  }

  private template(name: string): Node[] {
    let nodes = this.templates.get(name);
    if (nodes === undefined && this.loader) {
      const source = this.loader(name);
      if (source !== undefined) {
        nodes = parseTemplate(source);
        this.templates.set(name, nodes);
      }
    }
    if (nodes === undefined) throw new RenderError(`unknown template ${JSON.stringify(name)}`);
    return nodes;
  }

  private renderNamed(name: string, state: RenderState): string {
    if (state.active.includes(name)) {
      throw new RenderError(`include cycle: ${[...state.active, name].join(" -> ")}`);
    }
    const nodes = this.template(name);
    state.active.push(name);
    try {
      return this.renderNodes(nodes, state);
    } finally {
      state.active.pop();
    }
  }

  private renderNodes(nodes: Node[], state: RenderState): string {
    let out = "";
    for (const node of nodes) out += this.renderNode(node, state);
    return out;
  }

  private renderNode(node: Node, state: RenderState): string {
    switch (node.type) {
      case "text":
        return node.text;
      case "output": {
        const value = this.evaluate(node.expr, state);
        if (value === undefined && this.strict) {
          throw new RenderError(`line ${node.line}: undefined value in strict mode`);
        }
        const text = toText(value);
        return this.autoescape && !(value instanceof Safe) ? escapeHtml(text) : text;
      }
      case "if": {
        for (const branch of node.branches) {
          if (truthy(this.evaluate(branch.cond, state))) return this.renderNodes(branch.body, state);
        }
        return node.otherwise ? this.renderNodes(node.otherwise, state) : "";
      }
      case "for":
        return this.renderFor(node, state);
      case "include":
        return this.renderNamed(node.name, state);
    }
  }

  private renderFor(node: Extract<Node, { type: "for" }>, state: RenderState): string {
    const value = unwrap(this.evaluate(node.iterable, state));
    if (value === undefined && this.strict) {
      throw new RenderError(`line ${node.line}: loop over an undefined value in strict mode`);
    }
    let items: unknown[][];
    if (value === undefined || value === null) {
      items = [];
    } else if (node.names.length === 1 && Array.isArray(value)) {
      items = value.map((v) => [v]);
    } else if (node.names.length === 2 && isPlainObject(value)) {
      items = Object.entries(value);
    } else {
      throw new RenderError(`line ${node.line}: cannot loop over this value with ${node.names.length} variable(s)`);
    }
    if (items.length === 0) return node.otherwise ? this.renderNodes(node.otherwise, state) : "";
    let out = "";
    items.forEach((values, i) => {
      const scope: Scope = new Map();
      node.names.forEach((name, j) => scope.set(name, values[j]));
      scope.set("loop", { index: i + 1, index0: i, first: i === 0, last: i === items.length - 1, length: items.length });
      state.scopes.push(scope);
      try {
        out += this.renderNodes(node.body, state);
      } finally {
        state.scopes.pop();
      }
    });
    return out;
  }

  private lookup(name: string, state: RenderState): unknown {
    for (let i = state.scopes.length - 1; i >= 0; i--) {
      if (state.scopes[i].has(name)) return state.scopes[i].get(name);
    }
    return Object.hasOwn(state.data, name) ? state.data[name] : undefined;
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
