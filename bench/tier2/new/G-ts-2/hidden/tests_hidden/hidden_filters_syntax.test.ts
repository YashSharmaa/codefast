import { test } from "node:test";
import assert from "node:assert/strict";
import { Environment } from "../src/environment.ts";
import { RenderError, TemplateSyntaxError } from "../src/errors.ts";
import { builtinFilters } from "../src/filters.ts";

const r = (src: string, data: Record<string, unknown> = {}, opts = {}) => new Environment(opts).renderString(src, data);

test("upper, lower, trim use the output text", () => {
  assert.equal(r("{{ s | upper }}|{{ s | lower }}|{{ p | trim }}|{{ n | upper }}|{{ xs | upper }}|{{ missing | upper }}",
    { s: "MiXed", p: "  pad \n", n: 1.5, xs: ["a", "b"] }), "MIXED|mixed|pad|1.5|A,B|");
});

test("length", () => {
  assert.equal(r("{{ s | length }} {{ xs | length }} {{ o | length }} {{ nil | length }} {{ missing | length }}",
    { s: "four", xs: [1, 2, 3], o: { a: 1, b: 2 }, nil: null }), "4 3 2 0 0");
  assert.throws(() => r("{{ n | length }}", { n: 5 }), RenderError);
});

test("default", () => {
  assert.equal(r("{{ missing | default('d') }}|{{ nil | default('d') }}|{{ e | default('d') }}|{{ z | default('d') }}|{{ f | default(1) }}",
    { nil: null, e: "", z: 0, f: false }), "d|d||0|false");
  assert.equal(r("{{ missing | default(other) }}", { other: "<o>" }), "&lt;o&gt;");
});

test("join, first, last", () => {
  assert.equal(r("{{ xs | join }}|{{ xs | join(' - ') }}|{{ xs | first }}|{{ xs | last }}|{{ s | first }}{{ s | last }}|{{ e | first }}",
    { xs: [1, "b", true], s: "xyz", e: [] }), "1,b,true|1 - b - true|1|true|xz|");
  assert.throws(() => r("{{ s | join }}", { s: "abc" }), RenderError);
  assert.throws(() => r("{{ n | first }}", { n: 5 }), RenderError);
});

test("truncate and replace", () => {
  assert.equal(r("{{ s | truncate(5) }}|{{ s | truncate(11) }}|{{ s | truncate(20) }}|{{ s | truncate(0) }}", { s: "hello world" }),
    "hello...|hello world|hello world|...");
  assert.equal(r("{{ s | replace('o', '0') }}|{{ s | replace('xyz', '!') }}", { s: "foo boo" }), "f00 b00|foo boo");
  assert.throws(() => r("{{ s | truncate(-1) }}", { s: "abc" }), RenderError);
  assert.throws(() => r("{{ n | truncate(2) }}", { n: 12345 }), RenderError);
});

test("round", () => {
  assert.equal(r("{{ a | round }} {{ b | round }} {{ c | round(2) }} {{ d | round(1) }} {{ 7 | round }}", { a: 2.5, b: -2.5, c: 1.23456, d: 9.96 }),
    "3 -2 1.23 10 7");
  assert.throws(() => r("{{ s | round }}", { s: "1.5" }), RenderError);
});

test("sort returns a sorted copy", () => {
  assert.equal(r("{{ ns | sort | join(' ') }} / {{ ns | join(' ') }} / {{ ss | sort }}", { ns: [10, 9, 1.5, -1], ss: ["b", "B", "a"] }),
    "-1 1.5 9 10 / 10 9 1.5 -1 / B,a,b");
  assert.throws(() => r("{{ xs | sort }}", { xs: [1, "a"] }), RenderError);
  assert.throws(() => r("{{ s | sort }}", { s: "cba" }), RenderError);
});

test("unknown filters, arguments and custom filters", () => {
  assert.throws(() => r("{{ x | shout }}", { x: 1 }), RenderError);
  assert.equal(r("{% if false %}{{ x | shout }}{% endif %}ok"), "ok");
  assert.throws(() => r("{{ x | upper(1) }}", { x: "a" }), RenderError);
  const env = new Environment();
  const seen: unknown[] = [];
  env.addFilter("wrap", (v, a, b) => `${a}${String(v)}${b}`);
  env.addFilter("upper", (v) => `custom:${String(v)}`);
  env.addFilter("spy", (v) => (seen.push(v), v));
  assert.equal(env.renderString("{{ x | wrap('[', ']') }}|{{ x | upper }}|{{ y | spy }}", { x: "<a>", y: 3 }),
    "[&lt;a&gt;]|custom:&lt;a&gt;|3");
  env.renderString("{{ x | raw | spy }}", { x: "<b>" });
  assert.equal(seen[1], "<b>");
  for (const name of ["upper", "lower", "trim", "length", "default", "join", "first", "last", "truncate", "replace", "round", "sort", "raw"]) {
    assert.equal(typeof builtinFilters[name], "function", name);
  }
});

function syntaxLine(src: string): number {
  try {
    new Environment().renderString(src, {});
  } catch (e) {
    if (e instanceof TemplateSyntaxError) return e.line;
    throw e;
  }
  return -1;
}

test("syntax errors report the line of the tag", () => {
  const cases: Array<[string, number]> = [
    ["a\nb\n{{ x ", 3],
    ["a\n{% frobnicate %}", 2],
    ["{% endif %}", 1],
    ["{% if a %}\n{% endfor %}", 2],
    ["{% for x in xs %}{% else %}\n{% else %}{% endfor %}", 2],
    ["{% if a %}{% else %}\n\n{% elif b %}{% endif %}", 3],
    ["x\n{% elif a %}", 2],
    ["\n{{ }}", 2],
    ["{{ a b }}", 1],
    ["\n\n{{ (a }}", 3],
    ["{% for in xs %}{% endfor %}", 1],
    ["{% for x of xs %}{% endfor %}", 1],
    ["\n{% include name %}", 2],
    ["{{ 'abc }}", 1],
    ["\n{% if a %}\n{% for x in xs %}\n{% endfor %}\n", 2],
    ["{% if %}{% endif %}", 1],
    ["{# never closed", 1],
  ];
  for (const [src, line] of cases) assert.equal(syntaxLine(src), line, JSON.stringify(src));
});
