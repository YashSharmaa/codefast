// Command line: node src/cli.ts TEMPLATE DATA [--strict] [--no-escape]

import { readFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { TemplateError } from "./errors.ts";
import { Environment } from "./environment.ts";

const USAGE = "usage: node src/cli.ts TEMPLATE DATA [--strict] [--no-escape]";

function run(argv: string[]): number {
  const flags = argv.filter((a) => a.startsWith("--"));
  const files = argv.filter((a) => !a.startsWith("--"));
  if (files.length !== 2 || flags.some((f) => f !== "--strict" && f !== "--no-escape")) {
    process.stderr.write(USAGE + "\n");
    return 2;
  }
  const [templatePath, dataPath] = files;
  try {
    const data = JSON.parse(readFileSync(dataPath, "utf8"));
    if (data === null || typeof data !== "object" || Array.isArray(data)) {
      throw new Error("data must be a JSON object");
    }
    const dir = dirname(templatePath);
    const env = new Environment({
      strict: flags.includes("--strict"),
      autoescape: !flags.includes("--no-escape"),
      loader: (name) => readFileSync(join(dir, name), "utf8"),
    });
    process.stdout.write(env.render(basename(templatePath), data));
    return 0;
  } catch (err) {
    const message = err instanceof TemplateError ? err.describe() : err instanceof Error ? err.message : String(err);
    process.stderr.write(`error: ${message.replace(/\n/g, " ")}\n`);
    return 1;
  }
}

process.exitCode = run(process.argv.slice(2));
