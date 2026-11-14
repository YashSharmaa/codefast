// Builds the template tree from pieces, and expression trees from expression tokens.

import { TemplateSyntaxError } from "./errors.ts";
import { lexExpression, splitTemplate, type Piece, type Token } from "./lexer.ts";

export type Expr =
  | { type: "literal"; value: unknown }
  | { type: "path"; name: string; segments: string[] }
  | { type: "filter"; input: Expr; name: string; args: Expr[] }
  | { type: "not"; operand: Expr }
  | { type: "and" | "or"; left: Expr; right: Expr }
  | { type: "compare"; op: string; left: Expr; right: Expr }
  | { type: "arith"; op: string; left: Expr; right: Expr }
  | { type: "neg"; operand: Expr }
  | { type: "list"; items: Expr[] }
  | { type: "object"; entries: Array<[string, Expr]> }
  | { type: "index"; target: Expr; key: Expr }
  | { type: "attr"; target: Expr; segment: string }
  | CallExpr;

export type CallExpr = { type: "call"; callee: Expr; args: Expr[]; kwargs: Array<[string, Expr]> };

export type Node =
  | { type: "text"; text: string }
  | { type: "output"; expr: Expr; line: number }
  | { type: "if"; branches: Array<{ cond: Expr; body: Node[]; line: number }>; otherwise: Node[] | null }
  | { type: "for"; names: string[]; iterable: Expr; filter: Expr | null; body: Node[]; otherwise: Node[] | null; line: number }
  | { type: "include"; name: string; line: number }
  | { type: "set"; name: string; expr: Expr; line: number }
  | { type: "macro"; name: string; params: string[]; defaults: Array<Expr | null>; body: Node[] }
  | { type: "callblock"; call: CallExpr; body: Node[]; line: number }
  | { type: "import"; name: string; alias: string; line: number }
  | { type: "fromimport"; name: string; names: Array<[string, string]>; line: number }
  | { type: "break" | "continue" };

const KEYWORDS = new Set(["and", "or", "not", "in", "true", "false", "null"]);
const COMPARE = new Set(["==", "!=", "<", "<=", ">", ">="]);
const ADDITIVE = new Set(["+", "-"]);
const MULTIPLICATIVE = new Set(["*", "/", "//", "%"]);

class ExprParser {
  tokens: Token[];
  pos = 0;
  line: number;
  template: string | null;

  constructor(tokens: Token[], line: number, template: string | null) {
    this.tokens = tokens;
    this.line = line;
    this.template = template;
  }

  fail(message: string): never {
    throw new TemplateSyntaxError(message, { line: this.line, template: this.template });
  }

  peek(): Token | undefined {
    return this.tokens[this.pos];
  }

  isOp(value: string): boolean {
    const t = this.peek();
    return t !== undefined && t.type === "op" && t.value === value;
  }

  isWord(value: string): boolean {
    const t = this.peek();
    return t !== undefined && t.type === "name" && t.value === value;
  }

  expectOp(value: string): void {
    if (!this.isOp(value)) this.fail(`expected ${value}`);
    this.pos++;
  }

  name(): string {
    const t = this.peek();
    if (!t || t.type !== "name" || KEYWORDS.has(t.value as string)) this.fail("expected a name");
    this.pos++;
    return t.value as string;
  }

  parseAll(): Expr {
    if (this.tokens.length === 0) this.fail("empty expression");
    const e = this.or();
    if (this.pos < this.tokens.length) this.fail(`unexpected ${String(this.peek()!.value)}`);
    return e;
  }

  or(): Expr {
    let left = this.and();
    while (this.isWord("or")) {
      this.pos++;
      left = { type: "or", left, right: this.and() };
    }
    return left;
  }

  and(): Expr {
    let left = this.not();
    while (this.isWord("and")) {
      this.pos++;
      left = { type: "and", left, right: this.not() };
    }
    return left;
  }

  not(): Expr {
    if (this.isWord("not")) {
      this.pos++;
      return { type: "not", operand: this.not() };
    }
    return this.comparison();
  }

  comparison(): Expr {
    const left = this.concat();
    const t = this.peek();
    if (t && ((t.type === "op" && COMPARE.has(t.value as string)) || (t.type === "name" && t.value === "in"))) {
      this.pos++;
      return { type: "compare", op: t.value as string, left, right: this.concat() };
    }
    return left;
  }

  isOpIn(ops: Set<string>): string | undefined {
    const t = this.peek();
    return t !== undefined && t.type === "op" && ops.has(t.value as string) ? (t.value as string) : undefined;
  }

  concat(): Expr {
    let left = this.additive();
    while (this.isOp("~")) {
      this.pos++;
      left = { type: "arith", op: "~", left, right: this.additive() };
    }
    return left;
  }

  additive(): Expr {
    let left = this.multiplicative();
    for (let op = this.isOpIn(ADDITIVE); op; op = this.isOpIn(ADDITIVE)) {
      this.pos++;
      left = { type: "arith", op, left, right: this.multiplicative() };
    }
    return left;
  }

  multiplicative(): Expr {
    let left = this.unary();
    for (let op = this.isOpIn(MULTIPLICATIVE); op; op = this.isOpIn(MULTIPLICATIVE)) {
      this.pos++;
      left = { type: "arith", op, left, right: this.unary() };
    }
    return left;
  }

  unary(): Expr {
    if (this.isOp("-")) {
      this.pos++;
      return { type: "neg", operand: this.unary() };
    }
    return this.filtered();
  }

  postfix(): Expr {
    let target = this.atom();
    for (;;) {
      if (this.isOp("[")) {
        this.pos++;
        const key = this.or();
        this.expectOp("]");
        target = { type: "index", target, key };
      } else if (this.isOp("(")) {
        this.pos++;
        target = this.callArgs(target);
      } else if (this.isOp(".")) {
        this.pos++;
        target = { type: "attr", target, segment: this.segment() };
      } else {
        return target;
      }
    }
  }

  callArgs(callee: Expr): CallExpr {
    const args: Expr[] = [];
    const kwargs: Array<[string, Expr]> = [];
    while (!this.isOp(")")) {
      if (args.length + kwargs.length > 0) this.expectOp(",");
      const t = this.peek();
      const next = this.tokens[this.pos + 1];
      if (t && t.type === "name" && next && next.type === "op" && next.value === "=") {
        const key = this.name();
        this.pos++;
        if (kwargs.some(([k]) => k === key)) this.fail(`keyword argument ${key} given twice`);
        kwargs.push([key, this.or()]);
      } else {
        if (kwargs.length > 0) this.fail("positional argument after keyword argument");
        args.push(this.or());
      }
    }
    this.pos++;
    return { type: "call", callee, args, kwargs };
  }

  // The part of a for tag after "in": an expression, then optionally "if" and a condition.
  iterable(): { iterable: Expr; filter: Expr | null } {
    if (this.tokens.length === 0) this.fail("empty expression");
    const iterable = this.or();
    let filter: Expr | null = null;
    if (this.isWord("if")) {
      this.pos++;
      filter = this.or();
    }
    if (this.pos < this.tokens.length) this.fail(`unexpected ${String(this.peek()!.value)}`);
    return { iterable, filter };
  }

  // name(param, param = default, ...) of a macro tag, up to the end of the tokens.
  signature(): { name: string; params: string[]; defaults: Array<Expr | null> } {
    const name = this.name();
    this.expectOp("(");
    const params: string[] = [];
    const defaults: Array<Expr | null> = [];
    while (!this.isOp(")")) {
      if (params.length > 0) this.expectOp(",");
      const param = this.name();
      if (params.includes(param)) this.fail(`parameter ${param} given twice`);
      params.push(param);
      if (this.isOp("=")) {
        this.pos++;
        defaults.push(this.or());
      } else {
        if (defaults.some((d) => d !== null)) this.fail("a parameter without a default follows one with a default");
        defaults.push(null);
      }
    }
    this.pos++;
    if (this.pos < this.tokens.length) this.fail(`unexpected ${String(this.peek()!.value)}`);
    return { name, params, defaults };
  }

  segment(): string {
    const s = this.peek();
    if (!s || !(s.type === "seg" || (s.type === "name" && !KEYWORDS.has(s.value as string)))) {
      this.fail("expected a name or index after .");
    }
    this.pos++;
    return String(s.value);
  }

  list(close: string): Expr[] {
    const items: Expr[] = [];
    if (this.isOp(close)) return items;
    items.push(this.or());
    while (this.isOp(",")) {
      this.pos++;
      items.push(this.or());
    }
    return items;
  }

  filtered(): Expr {
    let input = this.postfix();
    while (this.isOp("|")) {
      this.pos++;
      const name = this.name();
      const args: Expr[] = [];
      if (this.isOp("(")) {
        this.pos++;
        args.push(...this.list(")"));
        this.expectOp(")");
      }
      input = { type: "filter", input, name, args };
    }
    return input;
  }

  atom(): Expr {
    const t = this.peek();
    if (!t) this.fail("expression ends too early");
    if (t.type === "str" || t.type === "num") {
      this.pos++;
      return { type: "literal", value: t.value };
    }
    if (t.type === "op" && t.value === "(") {
      this.pos++;
      const e = this.or();
      this.expectOp(")");
      return e;
    }
    if (t.type === "name") {
      const word = t.value as string;
      if (word === "true" || word === "false" || word === "null") {
        this.pos++;
        return { type: "literal", value: word === "null" ? null : word === "true" };
      }
      const name = this.name();
      const segments: string[] = [];
      while (this.isOp(".")) {
        this.pos++;
        segments.push(this.segment());
      }
      return { type: "path", name, segments };
    }
    if (t.type === "op" && t.value === "[") {
      this.pos++;
      const items = this.list("]");
      this.expectOp("]");
      return { type: "list", items };
    }
    if (t.type === "op" && t.value === "{") {
      this.pos++;
      const entries: Array<[string, Expr]> = [];
      while (!this.isOp("}")) {
        if (entries.length > 0) this.expectOp(",");
        const key = this.peek();
        if (!key || key.type !== "str") this.fail("an object key must be a string literal");
        this.pos++;
        this.expectOp(":");
        entries.push([key.value as string, this.or()]);
      }
      this.pos++;
      return { type: "object", entries };
    }
    return this.fail(`unexpected ${String(t.value)}`);
  }
}

export function parseExpression(text: string, line: number, template: string | null): Expr {
  return new ExprParser(lexExpression(text, line, template), line, template).parseAll();
}

type Frame = { kind: "root" | "if" | "for" | "macro" | "call"; line: number; node?: Node; current: Node[]; sawElse: boolean };

const ENDS: Record<string, Frame["kind"]> = { endif: "if", endfor: "for", endmacro: "macro", endcall: "call" };

// True when a break or continue here belongs to the body of a loop of the same template.
function insideLoop(stack: Frame[]): boolean {
  for (let i = stack.length - 1; i >= 0; i--) {
    const kind = stack[i].kind;
    if (kind === "for") return !stack[i].sawElse;
    if (kind === "macro" || kind === "call") return false;
  }
  return false;
}

export function parseTemplate(source: string, template: string | null): Node[] {
  const root: Frame = { kind: "root", line: 1, current: [], sawElse: false };
  const rootBody = root.current;
  const stack: Frame[] = [root];
  const top = () => stack[stack.length - 1];
  for (const piece of splitTemplate(source, template) as Piece[]) {
    if (piece.kind === "text") {
      top().current.push({ type: "text", text: piece.text });
      continue;
    }
    const { body, line } = piece;
    if (piece.kind === "output") {
      top().current.push({ type: "output", expr: parseExpression(body, line, template), line });
      continue;
    }
    const word = /^[A-Za-z]+/.exec(body)?.[0] ?? "";
    const rest = body.slice(word.length).trim();
    const frame = top();
    if (word === "if") {
      const node: Node = { type: "if", branches: [{ cond: parseExpression(rest, line, template), body: [], line }], otherwise: null };
      frame.current.push(node);
      stack.push({ kind: "if", line, node, current: node.branches[0].body, sawElse: false });
    } else if (word === "elif") {
      if (frame.kind !== "if") throw new TemplateSyntaxError("elif without if", { line, template });
      if (frame.sawElse) throw new TemplateSyntaxError("elif after else", { line, template });
      const node = frame.node as Extract<Node, { type: "if" }>;
      const branch = { cond: parseExpression(rest, line, template), body: [] as Node[], line };
      node.branches.push(branch);
      frame.current = branch.body;
    } else if (word === "else") {
      if (rest !== "") throw new TemplateSyntaxError("else takes nothing", { line, template });
      if (frame.kind !== "if" && frame.kind !== "for") throw new TemplateSyntaxError("else without if or for", { line, template });
      if (frame.sawElse) throw new TemplateSyntaxError("second else", { line, template });
      frame.sawElse = true;
      const node = frame.node as Extract<Node, { type: "if" | "for" }>;
      node.otherwise = [];
      frame.current = node.otherwise;
    } else if (Object.hasOwn(ENDS, word)) {
      if (rest !== "") throw new TemplateSyntaxError(`${word} takes nothing`, { line, template });
      if (frame.kind !== ENDS[word]) throw new TemplateSyntaxError(`${word} without ${ENDS[word]}`, { line, template });
      stack.pop();
    } else if (word === "break" || word === "continue") {
      if (rest !== "") throw new TemplateSyntaxError(`${word} takes nothing`, { line, template });
      if (!insideLoop(stack)) throw new TemplateSyntaxError(`${word} outside a loop`, { line, template });
      frame.current.push({ type: word });
    } else if (word === "macro") {
      const { name, params, defaults } = new ExprParser(lexExpression(rest, line, template), line, template).signature();
      const node: Node = { type: "macro", name, params, defaults, body: [] };
      frame.current.push(node);
      stack.push({ kind: "macro", line, node, current: node.body, sawElse: false });
    } else if (word === "call") {
      const call = parseExpression(rest, line, template);
      if (call.type !== "call") throw new TemplateSyntaxError("call needs a call expression", { line, template });
      const node: Node = { type: "callblock", call, body: [], line };
      frame.current.push(node);
      stack.push({ kind: "call", line, node, current: node.body, sawElse: false });
    } else if (word === "import") {
      const tokens = lexExpression(rest, line, template);
      const [what, as, alias] = tokens;
      if (tokens.length !== 3 || what.type !== "str" || as.type !== "name" || as.value !== "as" || alias.type !== "name" || KEYWORDS.has(alias.value as string)) {
        throw new TemplateSyntaxError("malformed import", { line, template });
      }
      frame.current.push({ type: "import", name: what.value as string, alias: alias.value as string, line });
    } else if (word === "from") {
      const m = /^(["'][\s\S]*?["'])\s+import\s+([\s\S]+)$/.exec(rest);
      const what = m ? lexExpression(m[1], line, template) : [];
      const names = m ? m[2].split(",").map((part) => /^\s*([A-Za-z_][A-Za-z0-9_]*)(?:\s+as\s+([A-Za-z_][A-Za-z0-9_]*))?\s*$/.exec(part)) : [];
      if (what.length !== 1 || what[0].type !== "str" || names.some((n) => !n || KEYWORDS.has(n[1]) || KEYWORDS.has(n[2] ?? n[1]))) {
        throw new TemplateSyntaxError("malformed from import", { line, template });
      }
      const pairs = names.map((n): [string, string] => [n![1], n![2] ?? n![1]]);
      frame.current.push({ type: "fromimport", name: what[0].value as string, names: pairs, line });
    } else if (word === "for") {
      const m = /^([A-Za-z_][A-Za-z0-9_]*)(?:\s*,\s*([A-Za-z_][A-Za-z0-9_]*))?\s+in\s+([\s\S]+)$/.exec(rest);
      if (!m || KEYWORDS.has(m[1]) || (m[2] !== undefined && KEYWORDS.has(m[2]))) {
        throw new TemplateSyntaxError("malformed for", { line, template });
      }
      const names = m[2] === undefined ? [m[1]] : [m[1], m[2]];
      const { iterable, filter } = new ExprParser(lexExpression(m[3], line, template), line, template).iterable();
      const node: Node = { type: "for", names, iterable, filter, body: [], otherwise: null, line };
      frame.current.push(node);
      stack.push({ kind: "for", line, node, current: node.body, sawElse: false });
    } else if (word === "set") {
      const m = /^([A-Za-z_][A-Za-z0-9_]*)\s*=(?!=)([\s\S]*)$/.exec(rest);
      if (!m || KEYWORDS.has(m[1])) throw new TemplateSyntaxError("malformed set", { line, template });
      frame.current.push({ type: "set", name: m[1], expr: parseExpression(m[2], line, template), line });
    } else if (word === "include") {
      const tokens = lexExpression(rest, line, template);
      if (tokens.length !== 1 || tokens[0].type !== "str") {
        throw new TemplateSyntaxError("include needs a string literal", { line, template });
      }
      frame.current.push({ type: "include", name: tokens[0].value as string, line });
    } else {
      throw new TemplateSyntaxError(`unknown statement ${JSON.stringify(word || body)}`, { line, template });
    }
  }
  if (stack.length > 1) {
    const open = top();
    throw new TemplateSyntaxError(`${open.kind} block is never closed`, { line: open.line, template });
  }
  return rootBody;
}
