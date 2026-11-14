import { test } from "node:test";
import assert from "node:assert/strict";
import { Environment } from "../src/environment.ts";
import { RenderError, TemplateSyntaxError } from "../src/errors.ts";

const r = (src: string, data: Record<string, unknown> = {}, opts = {}) => new Environment(opts).renderString(src, data);

test("if, elif, else", () => {
  const src = "{% if n > 10 %}big{% elif n > 5 %}mid{% elif n > 0 %}small{% else %}none{% endif %}";
  assert.deepEqual([12, 7, 1, 0].map((n) => r(src, { n })), ["big", "mid", "small", "none"]);
  assert.equal(r("{% if a %}A{% endif %}{% if b %}B{% endif %}", { a: 1, b: 0 }), "A");
});

test("truthiness", () => {
  const data = { e: [], o: {}, z: 0, s: "", n: null, zs: "0", l: [0], oo: { a: 0 }, f: 0.5 };
  const src = "{% if x %}T{% else %}F{% endif %}";
  const got = Object.keys(data).map((k) => r(src.replace("x", k), data)).join("");
  assert.equal(got, "FFFFFTTTT");
  assert.equal(r(src.replace("x", "missing")), "F");
});

test("and, or, not give booleans", () => {
  assert.equal(r("{{ 1 and 'x' }}|{{ 0 or '' }}|{{ not v }}|{{ v or 0 }}|{{ 0 and missing.x }}", { v: "s" }),
    "true|false|false|true|false");
});

test("comparisons", () => {
  assert.equal(r("{{ 2 < 10 }} {{ 'b' < 'a' }} {{ 'B' < 'a' }} {{ 3 >= 3 }} {{ 3 > 3 }} {{ x <= 1.5 }}", { x: 1.5 }),
    "true false true true false true");
  assert.equal(r("{{ '1' == 1 }} {{ 1 == 1 }} {{ null == missing }} {{ 'a' != 'a' }} {{ a == a }}", { a: [] }),
    "false true false false true");
  assert.throws(() => r("{{ 1 < '2' }}"), RenderError);
  assert.throws(() => r("{{ missing < 2 }}"), RenderError);
});

test("in", () => {
  assert.equal(r("{{ 2 in xs }} {{ '2' in xs }} {{ 'ell' in 'hello' }} {{ 'k' in obj }} {{ 'v' in obj }}",
    { xs: [1, 2], obj: { k: "v" } }), "true false true true false");
  assert.throws(() => r("{{ 1 in 5 }}"), RenderError);
  assert.throws(() => r("{{ 1 in missing }}"), RenderError);
});

test("precedence", () => {
  assert.equal(r("{{ not a == b }}", { a: 1, b: 1 }), "false");
  assert.equal(r("{{ t or t and f }}|{{ (t or t) and f }}", { t: true, f: false }), "true|false");
  assert.equal(r("{{ xs | length > 2 }}|{{ name | upper == 'ANN' }}", { xs: [1, 2, 3], name: "ann" }), "true|true");
  assert.equal(r("{{ not not 'x' }}"), "true");
});

test("for loops and loop variables", () => {
  const src = "{% for x in xs %}{{ loop.index }}/{{ loop.index0 }}/{{ loop.length }}:{{ x }}{% if loop.first %}F{% endif %}{% if loop.last %}L{% endif %} {% endfor %}";
  assert.equal(r(src, { xs: ["a", "b", "c"] }), "1/0/3:aF 2/1/3:b 3/2/3:cL ");
  assert.equal(r("{% for x in xs %}{% for y in ys %}{{ loop.index }}{{ x }}{{ y }} {% endfor %}{{ loop.index }};{% endfor %}",
    { xs: ["a", "b"], ys: [1, 2] }), "1a1 2a2 1;1b1 2b2 2;");
  assert.equal(r("{{ x }}{% for x in xs %}{{ x }}{% endfor %}{{ x }}{{ loop }}", { x: "o", xs: [1, 2] }), "o12o");
});

test("for over objects, empty and bad values", () => {
  assert.equal(r("{% for k, v in o %}{{ k }}={{ v }};{% endfor %}", { o: { b: 1, a: 2 } }), "b=1;a=2;");
  assert.equal(r("{% for x in xs %}x{% else %}empty{% endfor %}", { xs: [] }), "empty");
  assert.equal(r("{% for k, v in o %}x{% else %}empty{% endfor %}", { o: {} }), "empty");
  assert.equal(r("{% for k, v in xs %}{{ k }}:{{ v }}{% endfor %}", { xs: [1] }), "0:1");
  assert.equal(r("{% for x in s %}{{ x }}-{% endfor %}", { s: "abc" }), "a-b-c-");
  assert.equal(r("{% for x in o %}{{ x }}{% endfor %}", { o: { a: 1 } }), "a");
  for (const [src, data] of [["{% for x in n %}{% endfor %}", { n: 3 }], ["{% for k, v in n %}{% endfor %}", { n: 3 }]] as const) {
    assert.throws(() => r(src, data), RenderError, src);
  }
});

test("raw blocks", () => {
  assert.equal(r("a{% raw %}{{ x }}{% if %}{% endraw %}b", { x: 1 }), "a{{ x }}{% if %}b");
  assert.throws(() => r("line1\n{% raw %}{{ x }}"), (e: unknown) => e instanceof TemplateSyntaxError && e.line === 2);
});

test("includes share variables and detect cycles", () => {
  const env = new Environment({
    templates: { row: "<{{ x }}:{{ loop.index }}:{{ title }}>", self: "{% include 'self' %}", a: "a{% include 'b' %}", b: "b{% include 'a' %}" },
  });
  assert.equal(env.renderString("{% for x in xs %}{% include \"row\" %}{% endfor %}", { xs: ["p", "q"], title: "T" }),
    "<p:1:T><q:2:T>");
  assert.throws(() => env.render("self"), RenderError);
  assert.throws(() => env.render("a"), RenderError);
  assert.throws(() => env.renderString("{% include 'nope' %}"), RenderError);
  assert.equal(env.renderString("{% include 'row' %}{% include 'row' %}", { x: 1 }), "<1::><1::>");
});

test("loader, named templates and parse timing", () => {
  const asked: string[] = [];
  const env = new Environment({
    templates: { main: "[{% include 'part' %}]" },
    loader: (name) => {
      asked.push(name);
      return name === "part" ? "{{ v | upper }}" : name === "broken" ? "{% if %}" : undefined;
    },
  });
  assert.equal(env.render("main", { v: "ok" }), "[OK]");
  assert.equal(env.render("part", { v: "x" }), "X");
  assert.throws(() => env.render("ghost"), RenderError);
  assert.throws(() => env.render("broken"), TemplateSyntaxError);
  assert.ok(asked.includes("part") && asked.includes("ghost"));
  assert.throws(() => env.addTemplate("bad", "{% endif %}"), TemplateSyntaxError);
  assert.throws(() => new Environment({ templates: { bad: "{{ " } }), TemplateSyntaxError);
  env.addTemplate("later", "{{ v }}!");
  assert.equal(env.render("later", { v: 1 }), "1!");
  assert.equal(env.render("later"), "!");
});
