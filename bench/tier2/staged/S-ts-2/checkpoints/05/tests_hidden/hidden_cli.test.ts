import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = fileURLToPath(new URL("..", import.meta.url));

function cli(args: string[]) {
  const p = spawnSync(process.execPath, ["src/cli.ts", ...args], { cwd: ROOT, encoding: "utf8", timeout: 20000 });
  return { code: p.status, out: p.stdout, err: p.stderr };
}

function workspace(files: Record<string, string>): string {
  const dir = mkdtempSync(join(tmpdir(), "stencil-hidden-"));
  for (const [name, text] of Object.entries(files)) writeFileSync(join(dir, name), text);
  return dir;
}

test("cli renders with includes relative to the template", () => {
  const dir = workspace({
    "page.txt": "Hello {{ name }}!\n{% for i in items %}{% include \"item.txt\" %}{% endfor %}",
    "item.txt": "- {{ i | upper }}\n",
    "data.json": JSON.stringify({ name: "<Ann>", items: ["a", "b"] }),
  });
  try {
    const p = cli([join(dir, "page.txt"), join(dir, "data.json")]);
    assert.equal(p.code, 0, p.err);
    assert.equal(p.out, "Hello &lt;Ann&gt;!\n- A\n- B\n");
    const raw = cli([join(dir, "page.txt"), join(dir, "data.json"), "--no-escape"]);
    assert.equal(raw.out, "Hello <Ann>!\n- A\n- B\n");
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("cli strict mode and errors", () => {
  const dir = workspace({
    "t.txt": "{{ missing }}x",
    "bad.txt": "ok\n\n{% if %}",
    "data.json": "{}",
    "broken.json": "{nope",
  });
  try {
    const lax = cli([join(dir, "t.txt"), join(dir, "data.json")]);
    assert.deepEqual([lax.code, lax.out], [0, "x"]);
    const strict = cli([join(dir, "t.txt"), join(dir, "data.json"), "--strict"]);
    assert.equal(strict.code, 1);
    assert.equal(strict.out, "");
    assert.match(strict.err, /^error: /);
    const syntax = cli([join(dir, "bad.txt"), join(dir, "data.json")]);
    assert.equal(syntax.code, 1);
    assert.match(syntax.err, /^error: bad\.txt:3: /);
    assert.equal(syntax.err.trim().split("\n").length, 1);
    for (const args of [[join(dir, "t.txt"), join(dir, "broken.json")], [join(dir, "t.txt"), join(dir, "nothing.json")],
      [join(dir, "nothing.txt"), join(dir, "data.json")]]) {
      const p = cli(args);
      assert.equal(p.code, 1, args.join(" "));
      assert.match(p.err, /^error: /);
    }
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("cli usage errors", () => {
  for (const args of [[], ["only-one.txt"], ["a", "b", "c"]]) {
    const p = cli(args);
    assert.equal(p.code, 2, args.join(" "));
    assert.notEqual(p.err, "");
  }
});
