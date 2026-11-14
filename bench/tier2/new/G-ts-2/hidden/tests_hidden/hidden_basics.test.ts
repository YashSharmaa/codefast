import { test } from "node:test";
import assert from "node:assert/strict";
import { Environment } from "../src/environment.ts";
import { RenderError, TemplateError, TemplateSyntaxError } from "../src/errors.ts";

const r = (src: string, data: Record<string, unknown> = {}, opts = {}) => new Environment(opts).renderString(src, data);

test("text and values", () => {
  assert.equal(r("plain text\n"), "plain text\n");
  assert.equal(r("{{ n }}|{{ f }}|{{ t }}|{{ no }}|{{ nil }}|{{ missing }}|{{ list }}",
    { n: 42, f: 2.5, t: true, no: false, nil: null, list: [1, "a", [2, 3], null, true] }), "42|2.5|true|false|||1,a,2,3,,true");
  assert.equal(r("{{ 'single' }} {{ \"double\" }} {{ -3 }} {{ 1.50 }}"), "single double -3 1.5");
  assert.equal(r("{{ 'it\\'s' }}|{{ 'a\\nb' }}|{{ 'back\\\\slash' }}"), "it&#39;s|a\nb|back\\slash");
});

test("escaping, raw and autoescape off", () => {
  const data = { x: `<a href="q">&'</a>` };
  assert.equal(r("<b>{{ x }}</b>", data), "<b>&lt;a href=&quot;q&quot;&gt;&amp;&#39;&lt;/a&gt;</b>");
  assert.equal(r("{{ x | raw }}", data), data.x);
  assert.equal(r("{{ x | raw | upper }}", data), "&lt;A HREF=&quot;Q&quot;&gt;&amp;&#39;&lt;/A&gt;");
  assert.equal(r("<b>{{ x }}</b>", data, { autoescape: false }), `<b>${data.x}</b>`);
  assert.equal(r("{{ list }}", { list: ["<", ">"] }), "&lt;,&gt;");
});

test("paths", () => {
  const data = { user: { name: "Ann", tags: ["x", "y"], "": 1 }, items: [{ id: 7 }, { id: 8 }], s: "str" };
  assert.equal(r("{{ user.name }} {{ user.tags.1 }} {{ items.1.id }} {{ items.0.id }}", data), "Ann y 8 7");
  assert.equal(r("[{{ user.nope }}][{{ items.5.id }}][{{ nope.deeper }}][{{ s.length }}][{{ items.length }}]", data), "[][][][][]");
  assert.equal(r("[{{ user.toString }}][{{ user.constructor }}]", data), "[][]");
});

test("objects cannot be output", () => {
  assert.throws(() => r("{{ user }}", { user: { a: 1 } }), RenderError);
  assert.throws(() => r("{{ list }}", { list: [{ a: 1 }] }), RenderError);
});

test("comments and whitespace control", () => {
  assert.equal(r("a {# hidden {{ x }} #}b"), "a b");
  assert.equal(r("a   \n  {{- x -}}  \n  b", { x: "X" }), "aXb");
  assert.equal(r("<ul>\n  {%- for i in xs %}\n  <li>{{ i }}</li>\n  {%- endfor %}\n</ul>", { xs: [1, 2] }),
    "<ul>\n  <li>1</li>\n  <li>2</li>\n</ul>");
  assert.equal(r("x  {#- note -#}  y"), "xy");
  assert.equal(r("{% if true -%}\n   yes\n{%- endif %}!"), "yes!");
});

test("strict mode", () => {
  const strict = { strict: true };
  assert.throws(() => r("{{ missing }}", {}, strict), RenderError);
  assert.throws(() => r("{{ user.missing }}", { user: {} }, strict), RenderError);
  assert.throws(() => r("{% for x in missing %}{% endfor %}", {}, strict), RenderError);
  assert.equal(r("{{ missing | default('d') }}|{{ nil }}", { nil: null }, strict), "d|");
  assert.equal(r("{% if missing %}y{% else %}n{% endif %}", {}, strict), "n");
  assert.equal(r("{% for x in missing %}y{% else %}none{% endfor %}"), "none");
  assert.equal(r("{% for x in nil %}y{% else %}none{% endfor %}", { nil: null }, strict), "none");
});

test("error classes", () => {
  const e = new TemplateSyntaxError("bad", 4);
  assert.equal(e.line, 4);
  assert.equal(e.message, "bad");
  assert.ok(e instanceof TemplateError);
  assert.ok(new RenderError("x") instanceof TemplateError);
  assert.ok(new TemplateError("x") instanceof Error);
  assert.ok(!(new RenderError("x") instanceof TemplateSyntaxError));
});
