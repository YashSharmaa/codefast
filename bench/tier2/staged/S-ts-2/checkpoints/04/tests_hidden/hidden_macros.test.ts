import { test } from "node:test";
import assert from "node:assert/strict";
import { Environment } from "../src/environment.ts";
import { RenderError, TemplateSyntaxError } from "../src/errors.ts";
import { Macro } from "../src/macros.ts";

const r = (src: string, data: Record<string, unknown> = {}, opts = {}) => new Environment(opts).renderString(src, data);

const FORMS = "{% macro input(name, type='text') %}<input name=\"{{ name }}\" type=\"{{ type }}\">{% endmacro %}" +
  "{% set note = 'n' %}{% if true %}{% macro label() %}{{ note }}[{{ who }}]{% endmacro %}{% endif %}visible";

test("macros are defined and called", () => {
  assert.equal(r("{% macro hello(name) %}Hello {{ name }}!{% endmacro %}{{ hello('Ann') }} {{ hello(n) }}", { n: "Bob" }), "Hello Ann! Hello Bob!");
  assert.equal(r("a{% macro m() %}x{% endmacro %}b"), "ab");
  assert.equal(r("{% macro c(n) %}{% if n > 0 %}{{ n }}{{ c(n - 1) }}{% endif %}{% endmacro %}{{ c(3) }}"), "321");
  assert.throws(() => r("{{ m() }}{% macro m() %}{% endmacro %}"), RenderError);
});

test("defaults and keyword arguments", () => {
  assert.equal(r("{% macro m(a, b = a * 2, c = 'c') %}[{{ a }},{{ b }},{{ c }}]{% endmacro %}{{ m(1) }}{{ m(1, 5) }}{{ m(2, c='z') }}{{ m(b=3, a=4) }}"),
    "[1,2,c][1,5,c][2,4,z][4,3,c]");
  assert.equal(r("{% macro m(a) %}[{{ a }}]{% endmacro %}{{ m() }}"), "[]");
  assert.equal(r("{% macro m(a = [1]) %}{% set a = a + [2] %}{{ a }}{% endmacro %}{{ m() }};{{ m() }}"), "1,2;1,2");
});

test("macro output is not escaped again", () => {
  assert.equal(r("{% macro m(a) %}<b>{{ a }}</b>{% endmacro %}{{ m('<x>') }}|{{ m('y') | upper }}|{{ m(s | raw) }}", { s: "<i>" }),
    "<b>&lt;x&gt;</b>|&lt;B&gt;Y&lt;/B&gt;|<b><i></b>");
  assert.equal(r("{% macro m() %}&{% endmacro %}{{ m() ~ '&' }}"), "&amp;&amp;");
});

test("binding errors and bad calls", () => {
  const def = "{% macro m(a) %}{% endmacro %}";
  for (const src of ["{{ m(1, 2) }}", "{{ m(z=1) }}", "{{ m(1, a=2) }}", "{{ m }}", "{{ s() }}", "{{ missing() }}", "{{ m()() }}"]) {
    assert.throws(() => r(def + src, { s: "str" }), RenderError, src);
  }
});

test("syntax of macros and calls", () => {
  for (const src of ["{% macro m(a, a) %}{% endmacro %}", "{% macro m(a = 1, b) %}{% endmacro %}", "{% macro m %}{% endmacro %}",
    "{% macro m() x %}{% endmacro %}", "{% macro m() %}", "{% endmacro %}", "{% macro m() %}{% else %}{% endmacro %}",
    "{{ f(a=1, 2) }}", "{{ f(a=1, a=2) }}", "{{ f(1,) }}", "{% macro and() %}{% endmacro %}", "{% macro m(1) %}{% endmacro %}"]) {
    assert.throws(() => r(src), TemplateSyntaxError, src);
  }
  assert.throws(() => r("\n{% macro m() %}\n"), (e: unknown) => e instanceof TemplateSyntaxError && e.line === 2);
});

test("macros see their definition site, not the caller", () => {
  assert.equal(r("{% set v = 'outer' %}{% macro show() %}{{ v }}/{{ d }}{% endmacro %}{% for v in ['loop'] %}{{ show() }}{% endfor %}", { d: "D" }),
    "outer/D");
  assert.equal(r("{% macro show() %}[{{ x }}]{% endmacro %}{% for x in [1] %}{{ show() }}{% endfor %}"), "[]");
  const env = new Environment({ templates: { part: "{{ m(1) }}" } });
  assert.equal(env.renderString("{% macro m(a) %}{{ a }}{% endmacro %}{{ m(0) }}{% include 'part' %}"), "01");
});

test("call blocks and caller", () => {
  const list = "{% macro list(items) %}<ul>{% for i in items %}<li>{{ caller() }}{{ i }}</li>{% endfor %}</ul>{% endmacro %}";
  assert.equal(r(list + "{% set mark = '*' %}{% call list([1, 2]) %}{{ mark }}{% endcall %}"), "<ul><li>*1</li><li>*2</li></ul>");
  assert.equal(r("{% macro wrap() %}[{{ caller() }}]{% endmacro %}{% call wrap() %}<{{ x }}>{% endcall %}", { x: "&" }), "[<&amp;>]");
  assert.equal(r("{% macro skip() %}none{% endmacro %}{% call skip() %}{{ 1 / 0 }}{% endcall %}"), "none");
  assert.equal(r("{% macro two(a) %}{{ a }}{{ caller() }}{{ caller() }}{% endmacro %}{% for x in ['p'] %}{% call two(x) %}{{ x }}{% endcall %}{% endfor %}"), "ppp");
});

test("call block errors", () => {
  for (const src of ["{% call x %}{% endcall %}", "{% endcall %}", "{% call m() %}", "{% call 1 + 2 %}{% endcall %}"]) {
    assert.throws(() => r(src), TemplateSyntaxError, src);
  }
  for (const src of ["{% call s() %}{% endcall %}", "{% call f() %}{% endcall %}",
    "{% macro m() %}{{ caller(1) }}{% endmacro %}{% call m() %}{% endcall %}"]) {
    assert.throws(() => r(src, { s: "str", f: () => "x" }), RenderError, src);
  }
});

test("globals", () => {
  const env = new Environment({ globals: { site: "S", greet: (n: unknown) => `hi ${String(n)}` }, templates: { inc: "{{ site }}" } });
  assert.equal(env.renderString("{{ greet(name) }}|{{ site }}|{% include 'inc' %}", { name: "Bo" }), "hi Bo|S|S");
  assert.equal(env.renderString("{{ site }}", { site: "D" }), "D");
  assert.equal(env.renderString("{% for site in ['L'] %}{{ site }}{% endfor %}{% set site = 'V' %}{{ site }}"), "LV");
  env.addGlobal("site", "<T>");
  env.addGlobal("extra", 1);
  assert.equal(env.renderString("{% macro m() %}{{ site }}{{ extra }}{% endmacro %}{{ m() }}"), "&lt;T&gt;1");
  assert.equal(new Environment({ globals: { g: 0 }, strict: true }).renderString("{{ g }}"), "0");
});

test("calling functions", () => {
  class Boom extends Error {}
  const data = {
    fmt: (n: number) => n.toFixed(2),
    kind: (v: unknown) => typeof v,
    tag: () => "<b>",
    boom: () => { throw new Boom("no"); },
    count: (...xs: unknown[]) => xs.length,
  };
  assert.equal(r("{{ fmt(2) }}|{{ kind(x | raw) }}|{{ tag() }}|{{ count() }}{{ count(1, [2, 3]) }}", { ...data, x: "<" }), "2.00|string|&lt;b&gt;|02");
  assert.throws(() => r("{{ fmt(n=1) }}", data), RenderError);
  assert.throws(() => r("{{ boom() }}", data), Boom);
  assert.throws(() => r("{{ fmt }}", data), RenderError);
});

test("import as a namespace", () => {
  const env = new Environment({ templates: { forms: FORMS } });
  assert.equal(env.renderString("{% import 'forms' as f %}{{ f.input('q') }}{{ f.input('p', type='pw') }}|{{ f.label() }}|{{ f.note }}|{{ f | length }}", { who: "W" }),
    "<input name=\"q\" type=\"text\"><input name=\"p\" type=\"pw\">|n[]||2");
  const loaded = new Environment({ loader: (name) => (name === "lib" ? "{% macro twice(x) %}{{ x }}{{ x }}{% endmacro %}" : undefined) });
  assert.equal(loaded.renderString("{% import \"lib\" as lib %}{{ lib.twice('ab') }}"), "abab");
});

test("from import and import syntax", () => {
  const env = new Environment({ templates: { forms: FORMS } });
  assert.equal(env.renderString("{% from 'forms' import input, label as lab %}{{ input('x') }}{{ lab() }}"), "<input name=\"x\" type=\"text\">n[]");
  assert.throws(() => env.renderString("{% from 'forms' import nothing %}"), RenderError);
  assert.throws(() => env.renderString("{% from 'forms' import note %}"), RenderError);
  for (const src of ["{% import 'forms' %}", "{% import forms as f %}", "{% from 'forms' import %}", "{% from 'forms' import a as %}",
    "{% import 'forms' as and %}", "{% from forms import input %}", "{% import 'forms' as f g %}"]) {
    assert.throws(() => env.renderString(src), TemplateSyntaxError, src);
  }
});

test("import errors and macro values", () => {
  const env = new Environment({ templates: { a: "{% import 'b' as b %}", b: "{% from 'a' import x %}", forms: FORMS } });
  assert.throws(() => env.render("a"), RenderError);
  assert.throws(() => env.renderString("{% import 'nope' as n %}"), RenderError);
  env.addGlobal("info", (m: unknown) => (m instanceof Macro ? `${m.name}:${m.params.join("+")}` : "no"));
  assert.equal(env.renderString("{% macro box(w, h = 1) %}{% endmacro %}{% import 'forms' as f %}{{ info(box) }}|{{ info(f.input) }}|{{ info(1) }}"),
    "box:w+h|input:name+type|no");
});
