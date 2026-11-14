import { test } from "node:test";
import assert from "node:assert/strict";
import { Environment } from "../src/environment.ts";
import { RenderError, TemplateSyntaxError } from "../src/errors.ts";

const r = (src: string, data: Record<string, unknown> = {}, opts = {}) => new Environment(opts).renderString(src, data);

test("one variable over strings and objects", () => {
  assert.equal(r("{% for c in s %}{{ c }}.{% endfor %}", { s: "abc" }), "a.b.c.");
  assert.equal(r("{% for k in o %}{{ k }}{{ loop.index }}{% endfor %}", { o: { b: 1, a: 2 } }), "b1a2");
  assert.equal(r("{% for c in '' %}x{% else %}empty{% endfor %}|{% for k in {} %}x{% else %}none{% endfor %}"), "empty|none");
  assert.equal(r("{% for c in s %}{{ c }}{% endfor %}", { s: "<&>" }), "&lt;&amp;&gt;");
});

test("two variables over arrays and strings", () => {
  assert.equal(r("{% for i, x in xs %}{{ i }}={{ x }};{% endfor %}", { xs: ["p", "q"] }), "0=p;1=q;");
  assert.equal(r("{% for i, c in 'ab' %}{{ i }}{{ c }}{% endfor %}"), "0a1b");
  assert.equal(r("{% for i, x in xs %}{{ loop.index }}{{ i }}{% endfor %}{% for i, x in [] %}{% else %}E{% endfor %}", { xs: [7, 8] }), "1021E");
  assert.equal(r("{% for k, v in o %}{{ k }}={{ v }};{% endfor %}", { o: { b: 1, a: 2 } }), "b=1;a=2;");
});

test("other values still cannot be looped over", () => {
  for (const [src, data] of [["{% for x in n %}{% endfor %}", { n: 3 }], ["{% for x in b %}{% endfor %}", { b: true }],
    ["{% for k, v in n %}{% endfor %}", { n: 1.5 }], ["{% for x in f %}{% endfor %}", { f: () => 1 }]] as const) {
    assert.throws(() => r(src, data), RenderError, src);
  }
  assert.throws(() => r("{% for x in missing %}{% endfor %}", {}, { strict: true }), RenderError);
  assert.equal(r("{% for x in nil %}y{% else %}none{% endfor %}{% for x in missing %}y{% else %}none{% endfor %}", { nil: null }), "nonenone");
});

test("filtered loops", () => {
  assert.equal(r("{% for x in xs if x % 3 == 0 %}{{ x }}/{{ loop.length }}{% if not loop.last %},{% endif %}{% endfor %}", { xs: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9] }),
    "0/4,3/4,6/4,9/4");
  assert.equal(r("{% for k, v in o if v > 1 %}{{ k }}{% else %}none{% endfor %}", { o: { a: 1, b: 0 } }), "none");
  assert.equal(r("{% for k, v in o if v %}{{ k }}{{ loop.index }}{% endfor %}", { o: { a: 1, b: 0, c: 2 } }), "a1c2");
  assert.equal(r("{% for x in [1, 2] %}{% for y in [5, 6] if loop.index == 2 %}{{ x }}{{ y }} {% endfor %}{% endfor %}"), "25 26 ");
  assert.equal(r("{% for c in 'a-b-c' if c != '-' %}{{ c }}{% endfor %}"), "abc");
});

test("filter syntax", () => {
  for (const src of ["{% for x in xs if %}{% endfor %}", "{% for x in xs foo %}{% endfor %}", "{% for x in xs if a b %}{% endfor %}"]) {
    assert.throws(() => r(src), TemplateSyntaxError, src);
  }
});

test("revindex and revindex0", () => {
  assert.equal(r("{% for x in xs %}{{ loop.revindex }}{{ loop.revindex0 }} {% endfor %}", { xs: ["p", "q", "r"] }), "32 21 10 ");
  assert.equal(r("{% for x in xs if x != 'q' %}{{ loop.revindex }}{% endfor %}", { xs: ["p", "q", "r"] }), "21");
});

test("previtem and nextitem", () => {
  assert.equal(r("{% for x in xs %}[{{ loop.previtem }}|{{ loop.nextitem }}]{% endfor %}", { xs: ["p", "q", "r"] }), "[|q][p|r][q|]");
  assert.equal(r("{% for k, v in {\"a\": 1, \"b\": 2} %}<{{ loop.previtem }}{{ loop.nextitem }}>{% endfor %}"), "<2><1>");
  assert.equal(r("{% for x in [1, 2, 3, 4] if x != 2 %}{{ loop.previtem }}{% endfor %}"), "13");
  assert.equal(r("{% for x in [1] %}{{ loop.previtem == null }}{{ loop.nextitem == null }}{% endfor %}"), "falsefalse");
});

test("depth", () => {
  const env = new Environment({ templates: { inner: "{% for y in [1] %}{{ loop.depth }}{% endfor %}" } });
  assert.equal(env.renderString("{% for x in [1] %}{{ loop.depth }}{% for y in [1] %}{{ loop.depth }}{% include 'inner' %}{% endfor %}{{ loop.depth }}{% endfor %}{% include 'inner' %}"),
    "12311");
});

test("break and continue", () => {
  assert.equal(r("{% for x in xs %}{% if x == 1 %}{% continue %}{% endif %}{% if x == 4 %}{% break %}{% endif %}{{ x }}{% endfor %}!", { xs: [0, 1, 2, 3, 4, 5] }),
    "023!");
  assert.equal(r("{% for x in xs %}{% for y in xs %}{% if y == 'q' %}{% break %}{% endif %}{{ y }}{% endfor %}{{ x }};{% endfor %}", { xs: ["p", "q", "r"] }),
    "pp;pq;pr;");
  assert.equal(r("{% for x in [1, 2] %}a{% break %}b{% endfor %}|{% for x in [1, 2] %}{{ x }}{% continue %}z{% endfor %}"), "a|12");
  assert.equal(r("{% for x in [1, 2, 3] %}{% if x > 1 %}{% break %}{% endif %}{{ loop.last }}{% endfor %}"), "false");
  assert.equal(r("{% macro m() %}{% for x in [1, 2] %}{{ x }}{% break %}{% endfor %}{% endmacro %}{{ m() }}{{ m() }}"), "11");
});

test("where break and continue may appear", () => {
  for (const src of ["{% break %}", "{% if true %}{% continue %}{% endif %}", "{% for x in xs %}{% else %}{% continue %}{% endfor %}",
    "{% for x in xs %}{% macro m() %}{% break %}{% endmacro %}{% endfor %}", "{% for x in xs %}{% break now %}{% endfor %}",
    "{% macro m() %}{% endmacro %}{% for x in xs %}{% call m() %}{% break %}{% endcall %}{% endfor %}"]) {
    assert.throws(() => r(src), TemplateSyntaxError, src);
  }
  assert.throws(() => r("{% for x in xs %}\n\n{% if x %}{% break %}{% endif %}{% else %}\n{% break %}{% endfor %}"),
    (e: unknown) => e instanceof TemplateSyntaxError && e.line === 4);
  assert.throws(() => new Environment({ templates: { stop: "{% break %}" } }), TemplateSyntaxError);
});

test("range", () => {
  assert.equal(r("{{ range(3) }}|{{ range(2, 5) }}|{{ range(5, 0, -2) }}|{{ range(0) | length }}|{{ range(3, 1) | length }}|{{ range(0, 10, 4) }}"),
    "0,1,2|2,3,4|5,3,1|0|0|0,4,8");
  assert.equal(r("{% for i in range(1, 4) %}{{ i * i }} {% endfor %}"), "1 4 9 ");
  for (const src of ["{{ range() }}", "{{ range(1, 2, 3, 4) }}", "{{ range(1.5) }}", "{{ range('3') }}", "{{ range(1, 5, 0) }}"]) {
    assert.throws(() => r(src), RenderError, src);
  }
  assert.equal(r("{{ range }}", { range: "mine" }), "mine");
  assert.equal(new Environment({ globals: { range: () => "g" } }).renderString("{{ range(2) }}"), "g");
});
