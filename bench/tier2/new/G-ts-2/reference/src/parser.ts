// Builds the template tree from pieces, and expression trees from expression tokens.

import { TemplateSyntaxError } from "./errors.ts";
import { lexExpression, splitTemplate, type Piece, type Token } from "./lexer.ts";

export type Expr =
  | { type: "literal"; value: unknown }
  | { type: "path"; name: string; segments: string[] }
  | { type: "filter"; input: Expr; name: string; args: Expr[] }
  | { type: "not"; operand: Expr }
  | { type: "and" | "or"; left: Expr; right: Expr }
  | { type: "compare"; op: string; left: Expr; right: Expr };

export type Node =
  | { type: "text"; text: string }
  | { type: "output"; expr: Expr; line: number }
  | { type: "if"; branches: Array<{ cond: Expr; body: Node[] }>; otherwise: Node[] | null }
  | { type: "for"; names: string[]; iterable: Expr; body: Node[]; otherwise: Node[] | null; line: number }
  | { type: "include"; name: string; line: number };

const KEYWORDS = new Set(["and", "or", "not", "in", "true", "false", "null"]);
const COMPARE = new Set(["==", "!=", "<", "<=", ">", ">="]);

class ExprParser {
  tokens: Token[];
  pos = 0;
  line: number;

  constructor(tokens: Token[], line: number) {
    this.tokens = tokens;
    this.line = line;
  }

  fail(message: string): never {
    throw new TemplateSyntaxError(message, this.line);
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
    const left = this.filtered();
    const t = this.peek();
    if (t && ((t.type === "op" && COMPARE.has(t.value as string)) || (t.type === "name" && t.value === "in"))) {
      this.pos++;
      return { type: "compare", op: t.value as string, left, right: this.filtered() };
    }
    return left;
  }

  filtered(): Expr {
    let input = this.atom();
    while (this.isOp("|")) {
      this.pos++;
      const name = this.name();
      const args: Expr[] = [];
      if (this.isOp("(")) {
        this.pos++;
        if (!this.isOp(")")) {
          args.push(this.or());
          while (this.isOp(",")) {
            this.pos++;
            args.push(this.or());
          }
        }
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
        const s = this.peek();
        if (!s || !(s.type === "seg" || (s.type === "name" && !KEYWORDS.has(s.value as string)))) {
          this.fail("expected a name or index after .");
        }
        segments.push(String(s.value));
        this.pos++;
      }
      return { type: "path", name, segments };
    }
    return this.fail(`unexpected ${String(t.value)}`);
  }
}

export function parseExpression(text: string, line: number): Expr {
  return new ExprParser(lexExpression(text, line), line).parseAll();
}

type Frame = { kind: "root" | "if" | "for"; line: number; node?: Node; current: Node[]; sawElse: boolean };

export function parseTemplate(source: string): Node[] {
  const root: Frame = { kind: "root", line: 1, current: [], sawElse: false };
  const rootBody = root.current;
  const stack: Frame[] = [root];
  const top = () => stack[stack.length - 1];
  for (const piece of splitTemplate(source) as Piece[]) {
    if (piece.kind === "text") {
      top().current.push({ type: "text", text: piece.text });
      continue;
    }
    const { body, line } = piece;
    if (piece.kind === "output") {
      top().current.push({ type: "output", expr: parseExpression(body, line), line });
      continue;
    }
    const word = /^[A-Za-z]+/.exec(body)?.[0] ?? "";
    const rest = body.slice(word.length).trim();
    const frame = top();
    if (word === "if") {
      const node: Node = { type: "if", branches: [{ cond: parseExpression(rest, line), body: [] }], otherwise: null };
      frame.current.push(node);
      stack.push({ kind: "if", line, node, current: node.branches[0].body, sawElse: false });
    } else if (word === "elif") {
      if (frame.kind !== "if") throw new TemplateSyntaxError("elif without if", line);
      if (frame.sawElse) throw new TemplateSyntaxError("elif after else", line);
      const node = frame.node as Extract<Node, { type: "if" }>;
      const branch = { cond: parseExpression(rest, line), body: [] as Node[] };
      node.branches.push(branch);
      frame.current = branch.body;
    } else if (word === "else") {
      if (rest !== "") throw new TemplateSyntaxError("else takes nothing", line);
      if (frame.kind === "root") throw new TemplateSyntaxError("else without if or for", line);
      if (frame.sawElse) throw new TemplateSyntaxError("second else", line);
      frame.sawElse = true;
      const node = frame.node as Extract<Node, { type: "if" | "for" }>;
      node.otherwise = [];
      frame.current = node.otherwise;
    } else if (word === "endif" || word === "endfor") {
      if (rest !== "") throw new TemplateSyntaxError(`${word} takes nothing`, line);
      if (frame.kind !== word.slice(3)) throw new TemplateSyntaxError(`${word} without ${word.slice(3)}`, line);
      stack.pop();
    } else if (word === "for") {
      const m = /^([A-Za-z_][A-Za-z0-9_]*)(?:\s*,\s*([A-Za-z_][A-Za-z0-9_]*))?\s+in\s+([\s\S]+)$/.exec(rest);
      if (!m || KEYWORDS.has(m[1]) || (m[2] !== undefined && KEYWORDS.has(m[2]))) {
        throw new TemplateSyntaxError("malformed for", line);
      }
      const names = m[2] === undefined ? [m[1]] : [m[1], m[2]];
      const node: Node = { type: "for", names, iterable: parseExpression(m[3], line), body: [], otherwise: null, line };
      frame.current.push(node);
      stack.push({ kind: "for", line, node, current: node.body, sawElse: false });
    } else if (word === "include") {
      const tokens = lexExpression(rest, line);
      if (tokens.length !== 1 || tokens[0].type !== "str") {
        throw new TemplateSyntaxError("include needs a string literal", line);
      }
      frame.current.push({ type: "include", name: tokens[0].value as string, line });
    } else {
      throw new TemplateSyntaxError(`unknown statement ${JSON.stringify(word || body)}`, line);
    }
  }
  if (stack.length > 1) {
    const open = top();
    throw new TemplateSyntaxError(`${open.kind} block is never closed`, open.line);
  }
  return rootBody;
}
