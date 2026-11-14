import { test } from "node:test";
import assert from "node:assert/strict";
import { Environment } from "../src/environment.ts";

const r = (src: string, data: Record<string, unknown> = {}, opts = {}) => new Environment(opts).renderString(src, data);

const EMOJI = "\u{1F600}";
const NAIVE = `naïve ${EMOJI}`;

test("length counts code points", () => {
  assert.equal(r("{{ s | length }}|{{ e | length }}|{{ two | length }}|{{ '' | length }}|{{ 'four' | length }}",
    { s: NAIVE, e: EMOJI, two: EMOJI + EMOJI }), "7|1|2|0|4");
});

test("first and last of strings with surrogate pairs", () => {
  assert.equal(r("{{ a | first }}|{{ b | last }}|{{ s | last }}|{{ 'ab' | first }}{{ 'ab' | last }}", { a: `${EMOJI}x`, b: `x${EMOJI}`, s: NAIVE }),
    `${EMOJI}|${EMOJI}|${EMOJI}|ab`);
  assert.equal(r("{{ s | first | length }}|{{ '' | first }}", { s: EMOJI }), "1|");
});

test("truncate counts code points", () => {
  assert.equal(r("{{ two | truncate(1) }}|{{ s | truncate(7) }}|{{ s | truncate(6) }}|{{ two | truncate(2) }}",
    { two: EMOJI + EMOJI, s: NAIVE }), `${EMOJI}...|${NAIVE}|naïve ...|${EMOJI}${EMOJI}`);
});

test("truncate never splits a pair", () => {
  assert.equal(r("{{ s | truncate(3) }}|{{ s | truncate(1) }}|{{ s | truncate(0) }}", { s: `${EMOJI}a${EMOJI}b` }), `${EMOJI}a${EMOJI}...|${EMOJI}...|...`);
});

test("loops over strings give code points", () => {
  assert.equal(r("{% for c in s %}[{{ c }}]{% endfor %}", { s: `a${EMOJI}b` }), `[a][${EMOJI}][b]`);
  assert.equal(r("{% for c in s %}{% if loop.last %}{{ loop.length }}{% endif %}{% endfor %}", { s: NAIVE }), "7");
});

test("two variables over strings count characters", () => {
  assert.equal(r("{% for i, c in s %}{{ i }}{{ c }} {% endfor %}", { s: `${EMOJI}${EMOJI}x` }), `0${EMOJI} 1${EMOJI} 2x `);
  assert.equal(r("{% for c in s %}{{ loop.revindex }}{{ loop.nextitem }};{% endfor %}", { s: `${EMOJI}y` }), "2y;1;");
});

test("combining marks are separate code points", () => {
  const s = "é";
  assert.equal(r("{{ s | length }}|{% for c in s %}.{% endfor %}|{{ s | first }}", { s }), "2|..|e");
});

test("order, substrings and arrays are unchanged", () => {
  const tilde = "～";
  assert.equal(r("{{ xs | sort | join(' ') }}|{{ a < b }}|{{ e in s }}|{{ xs | length }}|{{ xs | first }}",
    { xs: [tilde, EMOJI], a: EMOJI, b: tilde, e: EMOJI, s: NAIVE }), `${EMOJI} ${tilde}|true|true|2|${tilde}`);
  assert.equal(r("{{ s | replace(e, 'x') }}|{{ s | upper }}", { s: NAIVE, e: EMOJI }), `naïve x|NAÏVE ${EMOJI}`);
});

test("string literals in templates count the same way", () => {
  assert.equal(r(`{{ '${EMOJI}b' | length }}|{{ '${EMOJI}b' | first }}|{% for c in '${EMOJI}${EMOJI}' %}{{ loop.index }}{% endfor %}`), `2|${EMOJI}|12`);
});
