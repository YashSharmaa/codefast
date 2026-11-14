// Splits template source into text and tag tokens, and expression text into expression tokens.

import { TemplateSyntaxError } from "./errors.ts";

export type Piece =
  | { kind: "text"; text: string }
  | { kind: "output" | "statement"; body: string; line: number };

const OPENERS: Record<string, string> = { "{{": "}}", "{%": "%}", "{#": "#}" };
const RAW_END = /\{%-?\s*endraw\s*-?%\}/g;

function lineAt(source: string, index: number): number {
  let line = 1;
  for (let i = 0; i < index; i++) if (source.charCodeAt(i) === 10) line++;
  return line;
}

export function splitTemplate(source: string, template: string | null): Piece[] {
  const pieces: Piece[] = [];
  let trimNext = false;
  let pos = 0;
  const pushText = (text: string) => {
    if (trimNext) text = text.replace(/^\s+/, "");
    trimNext = false;
    pieces.push({ kind: "text", text });
  };
  while (pos < source.length) {
    const open = source.slice(pos).search(/\{[{%#]/);
    if (open < 0) {
      pushText(source.slice(pos));
      break;
    }
    const start = pos + open;
    const opener = source.slice(start, start + 2);
    const closer = OPENERS[opener];
    const line = lineAt(source, start);
    const end = source.indexOf(closer, start + 2);
    if (end < 0) throw new TemplateSyntaxError(`tag ${opener} is never closed`, { line, template });
    let inner = source.slice(start + 2, end);
    let text = source.slice(pos, start);
    if (inner.startsWith("-")) {
      text = text.replace(/\s+$/, "");
      inner = inner.slice(1);
    }
    pushText(text);
    let trimAfter = false;
    if (inner.endsWith("-")) {
      trimAfter = true;
      inner = inner.slice(0, -1);
    }
    pos = end + 2;
    if (opener === "{%" && inner.trim() === "raw") {
      RAW_END.lastIndex = pos;
      const m = RAW_END.exec(source);
      if (!m) throw new TemplateSyntaxError("raw block is never closed", { line, template });
      trimNext = trimAfter;
      pushText(source.slice(pos, m.index));
      pos = m.index + m[0].length;
      trimNext = m[0].endsWith("-%}");
      continue;
    }
    if (opener !== "{#") {
      pieces.push({ kind: opener === "{{" ? "output" : "statement", body: inner.trim(), line });
    }
    trimNext = trimAfter;
  }
  return pieces.filter((p) => p.kind !== "text" || p.text !== "");
}

export type Token = { type: "str" | "num" | "name" | "op" | "seg"; value: string | number };

const OPS = ["==", "!=", "<=", ">=", "//", "<", ">", "|", "(", ")", ",", ".", "+", "-", "*", "/", "%", "~", "[", "]", "{", "}", ":", "="];
const NOT_VALUE_END = new Set(["and", "or", "not", "in"]);

function endsValue(token: Token | undefined): boolean {
  if (!token) return false;
  if (token.type === "op") return token.value === ")" || token.value === "]" || token.value === "}";
  return token.type !== "name" || !NOT_VALUE_END.has(token.value as string);
}

export function lexExpression(text: string, line: number, template: string | null): Token[] {
  const tokens: Token[] = [];
  let i = 0;
  while (i < text.length) {
    const ch = text[i];
    if (/\s/.test(ch)) {
      i++;
      continue;
    }
    const prev = tokens[tokens.length - 1];
    if (prev && prev.type === "op" && prev.value === "." && /[0-9]/.test(ch)) {
      const m = /^[0-9]+/.exec(text.slice(i))!;
      tokens.push({ type: "seg", value: m[0] });
      i += m[0].length;
      continue;
    }
    if (ch === "'" || ch === '"') {
      let out = "";
      let j = i + 1;
      for (;;) {
        if (j >= text.length) throw new TemplateSyntaxError("unterminated string", { line, template });
        const c = text[j];
        if (c === ch) break;
        if (c === "\\") {
          j++;
          if (j >= text.length) throw new TemplateSyntaxError("unterminated string", { line, template });
          out += text[j] === "n" ? "\n" : text[j];
        } else {
          out += c;
        }
        j++;
      }
      tokens.push({ type: "str", value: out });
      i = j + 1;
      continue;
    }
    const num = (endsValue(prev) ? /^[0-9]+(\.[0-9]+)?/ : /^-?[0-9]+(\.[0-9]+)?/).exec(text.slice(i));
    if (num) {
      tokens.push({ type: "num", value: Number(num[0]) });
      i += num[0].length;
      continue;
    }
    const name = /^[A-Za-z_][A-Za-z0-9_]*/.exec(text.slice(i));
    if (name) {
      tokens.push({ type: "name", value: name[0] });
      i += name[0].length;
      continue;
    }
    const op = OPS.find((o) => text.startsWith(o, i));
    if (!op) throw new TemplateSyntaxError(`unexpected character ${JSON.stringify(ch)}`, { line, template });
    tokens.push({ type: "op", value: op });
    i += op.length;
  }
  return tokens;
}
