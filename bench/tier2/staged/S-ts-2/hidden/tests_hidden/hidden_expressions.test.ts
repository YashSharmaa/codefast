import { test } from "node:test";
import assert from "node:assert/strict";
import { Environment } from "../src/environment.ts";
import { RenderError, TemplateSyntaxError } from "../src/errors.ts";

const r = (src: string, data: Record<string, unknown> = {}, opts = {}) => new Environment(opts).renderString(src, data);

test("arithmetic precedence and grouping", () => {
  assert.equal(r("{{ 1 + 2 * 3 }}|{{ (1 + 2) * 3 }}|{{ 10 - 3 - 2 }}|{{ 2 * 3 % 4 }}|{{ 7 / 2 }}|{{ 12 / 2 / 3 }}"), "7|9|5|2|3.5|2");
  assert.equal(r("{{ 1 + 1 == 2 }}|{{ not 1 - 1 }}|{{ 2 * 2 > 3 and 1 }}|{{ 3 in [1, 1 + 2] }}"), "true|true|true|true");
});

test("floor division and modulo", () => {
  assert.equal(r("{{ 7 // 2 }} {{ -7 // 2 }} {{ 7 % 3 }} {{ -7 % 3 }} {{ 7 % -3 }} {{ 5.5 // 2 }} {{ 6 // 3 }}"), "3 -4 1 2 -2 2 2");
});

test("minus after a value subtracts", () => {
  assert.equal(r("{{ a-1 }} {{ a -1 }} {{ a - 1 }} {{ a - -1 }} {{ -a }} {{ (a)-1 }} {{ 3-1 }} {{ xs.0-1 }} {{ 'x' ~ -1 }}", { a: 5, xs: [10] }),
    "4 4 4 6 -5 4 2 9 x-1");
  assert.equal(r("{{ -2.5 | round }}|{{ -x | round }}|{{ - x }}|{{ [-1, 2] | first }}", { x: 2.5 }), "-2|-3|-2.5|-1");
});

test("plus on strings and arrays, type errors", () => {
  assert.equal(r("{{ 'a' + 'b' }}|{{ [1] + [2, 3] }}|{{ (xs + []) | length }}|{{ 0.5 + 0.25 }}", { xs: [1, 2] }), "ab|1,2,3|2|0.75");
  for (const src of ["{{ 'a' + 1 }}", "{{ 1 - 'a' }}", "{{ missing + 1 }}", "{{ -'x' }}", "{{ [1] * 2 }}", "{{ null + 1 }}", "{{ -flag }}"]) {
    assert.throws(() => r(src, { flag: true }), RenderError, src);
  }
});

test("division by zero", () => {
  for (const src of ["{{ 1 / 0 }}", "{{ 1 // 0 }}", "{{ 1 % 0 }}", "{{ 1 / z }}"]) {
    assert.throws(() => r(src, { z: 0 }), RenderError, src);
  }
  assert.equal(r("{{ 0 / 5 }}|{{ 0 % 5 }}"), "0|0");
});

test("concatenation", () => {
  assert.equal(r("{{ 'a' ~ 1 + 2 }}|{{ missing ~ 'x' }}|{{ n ~ t ~ nil }}|{{ xs ~ '!' }}|{{ 'a' ~ 'b' | upper }}",
    { n: 1.5, t: true, nil: null, xs: [1, 2] }), "a3|x|1.5true|1,2!|aB");
  assert.equal(r("{{ '<' ~ x }}|{{ (x | raw) ~ '<' }}", { x: "&" }), "&lt;&amp;|&amp;&lt;");
  assert.throws(() => r("{{ o ~ 'x' }}", { o: { a: 1 } }), RenderError);
});

test("list and object literals", () => {
  assert.equal(r("{{ [1, 'a', [2]] }}|{{ [] | length }}|{{ {\"a\": 1, 'b': 2} | length }}|{{ {\"k\": 1, \"k\": 2}.k }}|{{ {} | length }}"),
    "1,a,2|0|2|2|0");
  assert.equal(r("{% for k, v in {\"x\": 1, \"y\": [n, n * 2]} %}{{ k }}{{ v }};{% endfor %}", { n: 3 }), "x1;y3,6;");
  assert.equal(r("{{ [1] == [1] }}|{{ [x, x] | join('-') }}|{{ [[1, 2], [3]].1.0 }}", { x: "<" }), "false|&lt;-&lt;|3");
});

test("literal syntax errors", () => {
  for (const src of ["{{ [1, ] }}", "{{ {\"a\" 1} }}", "{{ {a: 1} }}", "{{ [1 }}", "{{ 1 + }}", "{{ * 2 }}", "{{ {\"a\": 1,} }}", "{{ {1: 2} }}"]) {
    assert.throws(() => r(src), TemplateSyntaxError, src);
  }
  assert.equal(r("{{ [ ] | length }}"), "0");
});

test("subscripts", () => {
  const data = { xs: [{ id: 3 }, "b"], o: { k: "v", "1": "one" }, i: 1, key: "k", s: "str" };
  assert.equal(r("{{ xs[1] }}|{{ xs[i - 1].id }}|{{ o['k'] }}|{{ o[key] }}|{{ [[1, 2]][0][1] }}|{{ o['1'] }}", data), "b|3|v|v|2|one");
  assert.equal(r("[{{ xs['0'] }}][{{ o[1] }}][{{ xs[5] }}][{{ xs[-1] }}][{{ s[0] }}][{{ xs[0.5] }}][{{ missing[0] }}][{{ o['toString'] }}]", data),
    "[][][][][][][][]");
  assert.throws(() => r("{{ xs[5] }}", data, { strict: true }), RenderError);
});

test("set at the top level and in includes", () => {
  const env = new Environment({ templates: { show: "{{ v }}", setter: "{% set v = 'inner' %}{{ v }}" } });
  assert.equal(env.renderString("{% set v = 1 + 1 %}{{ v }}{% include 'show' %}{% include 'setter' %}{{ v }}"), "22inner2");
  assert.equal(env.renderString("{{ v }}{% set v = 'set' %}{{ v }}", { v: "data" }), "dataset");
  assert.equal(env.renderString("{% if true %}{% set z = 5 %}{% endif %}{{ z }}"), "5");
  assert.equal(env.renderString("{% set a = [1, 2] %}{% set b = a + [3] %}{{ a }}/{{ b }}"), "1,2/1,2,3");
});

test("set in loops lasts one iteration", () => {
  assert.equal(r("{% set t = 0 %}{% for x in [1, 2] %}{% set t = t + x %}{{ t }}{% endfor %}{{ t }}"), "120");
  assert.equal(r("{% for x in [1, 2] %}{% if loop.first %}{% set y = 'Y' %}{% endif %}[{{ y }}]{% endfor %}"), "[Y][]");
  assert.equal(r("{% for x in [1, 2] %}{% set x = x * 10 %}{{ x }}{% endfor %}{{ x }}", { x: "o" }), "1020o");
});

test("malformed set", () => {
  assert.equal(r("{% set x=1 %}{{ x }}"), "1");
  for (const src of ["{% set %}", "{% set x %}", "{% set x == 1 %}", "{% set 1 = 2 %}", "{% set and = 1 %}", "{% set x = %}"]) {
    assert.throws(() => r(src), TemplateSyntaxError, src);
  }
  assert.throws(() => r("a\n{% set x %}"), (e: unknown) => e instanceof TemplateSyntaxError && e.line === 2);
});
