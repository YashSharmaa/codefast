import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { Environment } from "../src/environment.ts";
import { RenderError, TemplateError, TemplateSyntaxError } from "../src/errors.ts";

function caught(fn: () => unknown): TemplateError {
  try {
    fn();
  } catch (e) {
    if (e instanceof TemplateError) return e;
    throw e;
  }
  throw new Error("no error");
}

const where = (e: TemplateError) => [e.constructor.name, e.template, e.line];

test("error constructors take a location", () => {
  const s = new TemplateSyntaxError("bad", { line: 4 });
  assert.deepEqual([s.message, s.line, s.template], ["bad", 4, null]);
  assert.equal(new TemplateSyntaxError("bad", { line: 2, template: "p" }).template, "p");
  const e = new RenderError("x");
  assert.deepEqual([e.message, e.line, e.template], ["x", null, null]);
  assert.deepEqual(where(new RenderError("x", { template: "t" })), ["RenderError", "t", null]);
  const t = new TemplateError("m", { line: 2, template: "page" });
  assert.deepEqual([t.line, t.template], [2, "page"]);
  assert.deepEqual([new TemplateError("m", { line: null, template: null }).line, new TemplateError("m", {}).template], [null, null]);
  assert.ok(s instanceof TemplateError && e instanceof TemplateError && t instanceof Error && !(e instanceof TemplateSyntaxError));
});

test("describe", () => {
  assert.equal(new RenderError("boom", { template: "page", line: 3 }).describe(), "page:3: boom");
  assert.equal(new RenderError("boom", { template: "page" }).describe(), "page: boom");
  assert.equal(new TemplateSyntaxError("bad", { line: 7 }).describe(), "line 7: bad");
  assert.equal(new TemplateError("plain").describe(), "plain");
});

test("syntax errors know their template", () => {
  const env = new Environment({ loader: (name) => (name === "lbad" ? "\n\n{% endif %}" : undefined) });
  assert.deepEqual(where(caught(() => env.addTemplate("page", "a\n{% if %}"))), ["TemplateSyntaxError", "page", 2]);
  assert.deepEqual(where(caught(() => new Environment({ templates: { t: "{{ (a }}" } }))), ["TemplateSyntaxError", "t", 1]);
  assert.deepEqual(where(caught(() => env.render("lbad"))), ["TemplateSyntaxError", "lbad", 3]);
  assert.deepEqual(where(caught(() => env.renderString("{% include 'lbad' %}"))), ["TemplateSyntaxError", "lbad", 3]);
  const e = caught(() => env.renderString("\n\n\n{{ 'abc }}"));
  assert.deepEqual(where(e), ["TemplateSyntaxError", null, 4]);
  assert.equal(e.describe(), `line 4: ${e.message}`);
  assert.ok(!e.message.startsWith("line"));
});

test("render errors get the line of the tag", () => {
  const env = new Environment({ strict: true });
  const e = caught(() => env.renderString("a\n\n{{ missing }}"));
  assert.deepEqual(where(e), ["RenderError", null, 3]);
  assert.ok(!e.message.startsWith("line"));
  assert.deepEqual(where(caught(() => env.renderString("{% if 0 %}\n{% elif 1 < 'a' %}{% endif %}"))), ["RenderError", null, 2]);
  assert.deepEqual(where(caught(() => env.renderString("\n{% for x in 3 %}{% endfor %}"))), ["RenderError", null, 2]);
  assert.deepEqual(where(caught(() => env.renderString("{% for x in [1] %}\n{{ x | nope }}{% endfor %}"))), ["RenderError", null, 2]);
  assert.deepEqual(where(caught(() => env.renderString("\n\n{% set x = 1 / 0 %}"))), ["RenderError", null, 3]);
  assert.deepEqual(where(caught(() => env.renderString("{% for x in [1, 0] if 1 // x %}\n{% endfor %}"))), ["RenderError", null, 1]);
});

test("the line is where the tag starts", () => {
  const env = new Environment({ templates: { multi: "x\n{{\n  1 /\n  0 }}", raw: "{% raw %}\n{{ a }}\n{% endraw %}\n{{ 1 < 'a' }}" } });
  assert.deepEqual(where(caught(() => env.render("multi"))), ["RenderError", "multi", 2]);
  assert.deepEqual(where(caught(() => env.render("raw"))), ["RenderError", "raw", 4]);
});

test("errors inside includes", () => {
  const env = new Environment({
    templates: { page: "a\n{% include 'row' %}", row: "x\n\n{{ 1 < 'a' }}", missing: "\n\n{% include 'ghost' %}", cyc: "\n{% include 'cyc' %}" },
  });
  assert.deepEqual(where(caught(() => env.render("page"))), ["RenderError", "row", 3]);
  assert.equal(caught(() => env.render("page")).describe().slice(0, 6), "row:3:");
  assert.deepEqual(where(caught(() => env.render("missing"))), ["RenderError", "missing", 3]);
  assert.deepEqual(where(caught(() => env.render("cyc"))), ["RenderError", "cyc", 2]);
  assert.deepEqual(where(caught(() => env.render("ghost"))), ["RenderError", null, null]);
  assert.deepEqual(where(caught(() => env.renderString("ok\n{% include 'ghost' %}"))), ["RenderError", null, 2]);
});

test("errors in macro bodies and call blocks", () => {
  const env = new Environment({
    templates: { m: "{% macro boom() %}\n{{ 1 / 0 }}{% endmacro %}", lib: "{% macro wrap() %}\n[{{ caller() }}]{% endmacro %}" },
  });
  assert.deepEqual(where(caught(() => env.renderString("{% import 'm' as m %}\n\n{{ m.boom() }}"))), ["RenderError", "m", 2]);
  assert.deepEqual(where(caught(() => env.renderString("{% macro f(a) %}{% endmacro %}\n{{ f(1, 2) }}"))), ["RenderError", null, 2]);
  assert.deepEqual(where(caught(() => env.renderString("{% macro f(a = 1 // 0) %}\n{% endmacro %}\n\n{{ f() }}"))), ["RenderError", null, 4]);
  assert.deepEqual(where(caught(() => env.renderString("{% from 'lib' import wrap %}{% call wrap() %}\n\n{{ 1 // 0 }}{% endcall %}"))), ["RenderError", null, 3]);
  assert.deepEqual(where(caught(() => env.renderString("\n{% import 'nope' as n %}"))), ["RenderError", null, 2]);
  assert.deepEqual(where(caught(() => env.renderString("\n\n{% from 'm' import zzz %}"))), ["RenderError", null, 3]);
});

test("errors from custom filters and functions", () => {
  const env = new Environment({ templates: { t: "\n{{ x | bad }}", u: "\n\n{{ x | placed }}", v: "{{ f() }}" } });
  env.addFilter("bad", () => { throw new RenderError("nope"); });
  env.addFilter("placed", () => { throw new RenderError("kept", { line: 9, template: "elsewhere" }); });
  env.addGlobal("f", () => { throw new RenderError("from f"); });
  env.addGlobal("g", () => { throw new TypeError("plain"); });
  assert.deepEqual(where(caught(() => env.render("t"))), ["RenderError", "t", 2]);
  assert.deepEqual(where(caught(() => env.render("u"))), ["RenderError", "elsewhere", 9]);
  assert.deepEqual(where(caught(() => env.render("v"))), ["RenderError", "v", 1]);
  assert.throws(() => env.renderString("{{ g() }}"), (e: unknown) => e instanceof TypeError && !(e instanceof TemplateError));
});

const ROOT = fileURLToPath(new URL("..", import.meta.url));

function cli(args: string[]) {
  const p = spawnSync(process.execPath, ["src/cli.ts", ...args], { cwd: ROOT, encoding: "utf8", timeout: 20000 });
  return { code: p.status, out: p.stdout, err: p.stderr };
}

test("cli prints the template and line", () => {
  const dir = mkdtempSync(join(tmpdir(), "stencil-hidden-"));
  const files: Record<string, string> = {
    "bad.txt": "ok\n\n{% if %}",
    "page.txt": "top\n{% include 'row.txt' %}",
    "row.txt": "\n{{ 1 / 0 }}",
    "t.txt": "{{ missing }}",
    "lib.txt": "{% macro m() %}\n\n{{ 1 < 'a' }}{% endmacro %}",
    "use.txt": "{% import 'lib.txt' as lib %}{{ lib.m() }}",
    "data.json": "{}",
  };
  for (const [name, text] of Object.entries(files)) writeFileSync(join(dir, name), text);
  try {
    const cases: Array<[string[], RegExp]> = [
      [[join(dir, "bad.txt"), join(dir, "data.json")], /^error: bad\.txt:3: \S/],
      [[join(dir, "page.txt"), join(dir, "data.json")], /^error: row\.txt:2: \S/],
      [[join(dir, "t.txt"), join(dir, "data.json"), "--strict"], /^error: t\.txt:1: \S/],
      [[join(dir, "use.txt"), join(dir, "data.json")], /^error: lib\.txt:3: \S/],
    ];
    for (const [args, pattern] of cases) {
      const p = cli(args);
      assert.equal(p.code, 1, args.join(" "));
      assert.equal(p.out, "");
      assert.match(p.err, pattern);
      assert.equal(p.err.trim().split("\n").length, 1);
    }
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("locations do not change rendering", () => {
  const env = new Environment({ templates: { a: "{% for x in xs %}{{ x }}{% endfor %}" } });
  assert.equal(env.render("a", { xs: [1, 2] }), "12");
  assert.deepEqual(where(caught(() => env.render("a", { xs: 5 }))), ["RenderError", "a", 1]);
  assert.equal(env.render("a", { xs: ["ok"] }), "ok");
});
