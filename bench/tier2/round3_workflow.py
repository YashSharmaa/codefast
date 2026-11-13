# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Write the workflow script that runs one round-3 configuration (see round3.py).

usage: uv run bench/tier2/round3_workflow.py --tag r3h --model haiku
       -> target/ab/r3/<tag>.workflow.js
"""
import argparse, json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "target/ab/r3"

TEMPLATE = r"""export const meta = {
  name: 'agent-ab-round3',
  description: 'Paired runs of code-changing tasks (make a failing test pass, carry a rename through its call sites), each with and without the codefast command, every run in its own copy of the project',
  phases: [
    { title: 'Change', detail: 'one run per task and arm' },
  ],
}

const cfg = __CFG__
const { tag, model, guide, rule } = cfg

const SCHEMA = {
  type: 'object',
  properties: {
    done: { type: 'boolean', description: 'true when the check passes' },
    answer: { type: 'string', description: 'one or two sentences on what was changed' },
  },
  required: ['done', 'answer'],
}

const prompt = (t) => {
  let p
  if (t.family === 'D') {
    p = `You are working in a checkout of the ${t.repo_name} project. Your copy is at ${t.repo}.

This issue was reported against it:

--- issue ---
${t.issue}
--- end of issue ---

The tests that check the fix are already in the tree, and they fail. Change the source code so that they pass.

Failing tests: ${t.f2p.join(', ')}
How to run them: \`${t.test_cmd}\`

Rules: work only inside ${t.repo}. Do not read, list or run anything outside it, apart from the Python interpreter named in the command. Do not edit these test files: ${t.test_files.join(', ')}. Do not use the web, do not start sub-agents or workflows, and do not install anything. Stop as soon as the command passes.`
  } else {
    p = `You are working in a Go project: syft, a tool that lists the software packages inside container images and file systems. Your copy is at ${t.repo}.

Rename the method ${t.name} of ${t.receiver} (defined in ${t.file}) to ${t.new_name}: the definition and every place that calls it, tests included. Other types have a method called ${t.name} too: those are different methods and must stay as they are.

How to check: \`${t.check_cmd}\` (the first run takes about a minute). It has to pass when you are done.

Rules: work only inside ${t.repo}. Do not read, list or run anything outside it. Change nothing but the name of this one method and the calls of it. Do not use the web, do not start sub-agents or workflows. Stop as soon as the check passes.`
  }
  if (t.arm !== 'control') {
    p += `

A code-intelligence command is installed for this project: ${t.tool} (it is already pointed at your copy and keeps its index up to date by itself, so it needs no setup and no --root option). Wherever its guide below says \`codefast\`, run ${t.tool} instead.

--- guide ---
${guide}
--- end of guide ---

The project's instructions for assistants include this section:

--- project instructions ---
${rule}
--- end of project instructions ---`
  }
  p += `

When you are done, return the structured result: "done" says whether the check passes, and "answer" is one or two sentences on what you changed.`
  return p
}

phase('Change')
const results = await parallel(cfg.tasks.map((t) => () =>
  agent(prompt(t), { label: `${tag}:${t.id}:${t.arm}`, phase: 'Change', schema: SCHEMA, agentType: 'general-purpose', model, effort: 'medium' })
    .then(r => ({ id: t.id, arm: t.arm, result: r }))))
return { tag, model, results: results.filter(Boolean) }
"""


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--tag", required=True)
    ap.add_argument("--model", required=True, choices=["haiku", "sonnet", "opus"])
    a = ap.parse_args()
    cfg = json.load(open(OUT / f"{a.tag}.json"))
    cfg["model"] = a.model
    out = OUT / f"{a.tag}.workflow.js"
    out.write_text(TEMPLATE.replace("__CFG__", json.dumps(cfg)))
    print(out, f"({len(cfg['tasks'])} runs, model {a.model}, {out.stat().st_size // 1024} KB)")


if __name__ == "__main__":
    main()
