# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""codefast as one more arm of a published agent benchmark: agent-context-bench.

"How much does context cost an AI coding agent? Grep vs graph vs LSP, measured across 936 runs"
(Rybalchenko, 2026-06-24, https://github.com/Neko1313/agent-context-bench) runs Claude Code on
26 questions about apache/superset 6.0.0. Each arm has exactly one MCP server and no built-in
tool, the system prompt says the tools must be used, and an answer is graded against a key.
The repository publishes the tasks, the harness and every run.

This runs the same protocol with codefast's MCP server as the arm: the same command line (read
from that repository's `tests/runner.py` and `tests/arms.py`, which are not copied here), the
same system prompt, the same limit of turns, three runs per task, and the grading rule of its
`main.py`. `score` then sets the rows next to the published rows of the same model.

Two things differ, because the login here is not a credentials file that could be copied into
an empty configuration directory: the session runs with `--setting-sources project` in an
empty working directory, so that nothing of the user's configuration is loaded, and the
account's cloud connectors are switched off. `run` prints the tools the first session was
given, and stops if one of them is not the arm's.

usage: python3 -I bench/acb/run_acb.py fetch
       python3 -I bench/acb/run_acb.py run --arm codefast --out bench/results/acb-codefast-haiku.jsonl
       python3 -I bench/acb/run_acb.py score bench/results/acb-codefast-haiku.jsonl
"""
import argparse
import ast
import collections
import json
import os
import re
import statistics
import subprocess
import sys
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
THEIRS = ROOT / "bench/tools/acb/repo"
PINNED = "b500ade"
SUPERSET = ROOT / "bench/repos/superset"
MODELS = {"haiku": "claude-haiku-4-5-20251001", "sonnet": "claude-sonnet-4-6", "opus": "claude-opus-4-8"}
HARD = {"disambiguate", "overrides_count", "impact_set"}
# Variables a parent session leaves in the environment, which a child session must not see.
SCRUB = ("CLAUDECODE", "CLAUDE_CODE_", "CLAUDE_PID", "CLAUDE_EFFORT", "ANTHROPIC_API_KEY", "CODEFAST_")


def constant(path, name):
    """A module-level constant of the published harness, read without running the module."""
    for node in ast.parse(Path(path).read_text()).body:
        if isinstance(node, ast.Assign) and any(isinstance(t, ast.Name) and t.id == name for t in node.targets):
            return ast.literal_eval(node.value)
    raise KeyError(f"{name} in {path}")


def fetch(_):
    if not THEIRS.is_dir():
        THEIRS.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(["git", "clone", "-q", "https://github.com/Neko1313/agent-context-bench.git", str(THEIRS)], check=True)
    head = subprocess.run(["git", "-C", str(THEIRS), "rev-parse", "--short", "HEAD"], capture_output=True, text=True).stdout.strip()
    print(f"agent-context-bench at {head}" + ("" if head.startswith(PINNED) else f" (these figures were made at {PINNED})"))
    if not SUPERSET.is_dir():
        subprocess.run(["git", "clone", "-q", "--depth", "1", "--branch", "6.0.0", "https://github.com/apache/superset", str(SUPERSET)], check=True)
    print("apache/superset", subprocess.run(["git", "-C", str(SUPERSET), "describe", "--tags"], capture_output=True, text=True).stdout.strip())


def score_answer(answer, expected):
    """The grading rule of the published `main.py`: for a set, F1 of what the answer lists (paths
    ending in .py, or CamelCase names) against the key, a key item counting as found when the
    answer contains it. Otherwise the share of the expected strings the answer contains."""
    low = answer.lower()
    gold = {g.lower() for g in expected.get("answer_set") or []}
    if gold:
        found = {g for g in gold if g in low}
        pattern = r"[\w./-]+\.py" if any("/" in g for g in gold) else r"\b[A-Z][a-z0-9]+(?:[A-Z][a-z0-9]+)+\b"
        listed = {m.lower() for m in re.findall(pattern, answer)}
        tp, fp, fn = len(found), len(listed - gold), len(gold - found)
        precision = tp / (tp + fp) if tp + fp else 1.0
        recall = tp / (tp + fn) if tp + fn else 0.0
        return 2 * precision * recall / (precision + recall) if precision + recall else 0.0
    wanted = [w.lower() for w in expected.get("answer_contains") or []]
    return sum(w in low for w in wanted) / len(wanted) if wanted else 0.0


def arm_server(arm, binary):
    if arm == "codefast":
        return {"type": "stdio", "command": str(binary), "args": ["mcp", "--root", str(SUPERSET)]}
    sys.exit(f"no such arm: {arm}")


def one(a, task, rep, system, deny, turns, workdir, config):
    env = {k: v for k, v in os.environ.items() if not k.startswith(SCRUB)}
    env.update(CLAUDE_CONFIG_DIR=a.config_dir, ENABLE_CLAUDEAI_MCP_SERVERS="false", MCP_TIMEOUT="30000", MCP_TOOL_TIMEOUT="60000")
    cmd = [
        a.claude, "-p", task["prompt"], "--strict-mcp-config", "--mcp-config", config, "--allowedTools", f"mcp__{a.arm}",
        "--disallowedTools", *deny, "--system-prompt", system, "--model", MODELS[a.model], "--max-turns", str(turns),
        "--no-session-persistence", "--output-format", "stream-json", "--verbose", "--setting-sources", "project", "--no-chrome",
    ]
    out: dict = {}
    answer, used = "__CLI_ERROR__", []
    for attempt in range(4):  # a run in which no tool was called is run again, up to three times
        t0 = time.perf_counter()
        try:
            stdout = subprocess.run(cmd, cwd=workdir, env=env, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=a.timeout).stdout
        except subprocess.TimeoutExpired:
            stdout = ""  # no result line: an error, as in the published harness
        out = dict(input_tokens=0, output_tokens=0, cache_read_tokens=0, cache_creation_tokens=0, cost_usd=0.0, num_turns=0, given=[])
        answer, used = "__CLI_ERROR__", []
        for line in stdout.splitlines():
            try:
                ev = json.loads(line)
            except ValueError:
                continue
            if ev.get("type") == "system" and ev.get("subtype") == "init":
                out["given"] = ev.get("tools") or []
            elif ev.get("type") == "assistant":
                used += [b.get("name", "?") for b in ev.get("message", {}).get("content", []) if b.get("type") == "tool_use"]
            elif ev.get("type") == "result":
                u = ev.get("usage") or {}
                answer = str(ev.get("result", ""))
                out.update(input_tokens=int(u.get("input_tokens", 0)), output_tokens=int(u.get("output_tokens", 0)),
                           cache_read_tokens=int(u.get("cache_read_input_tokens", 0)), cache_creation_tokens=int(u.get("cache_creation_input_tokens", 0)),
                           cost_usd=float(ev.get("total_cost_usd", 0.0)), num_turns=int(ev.get("num_turns", 0)))
                if ev.get("is_error"):
                    answer = "__LIMIT_EXCEEDED__" if ev.get("subtype") == "error_max_turns" else "__CLI_ERROR__"
        out["wall_s"] = round(time.perf_counter() - t0, 2)
        out["retries"] = attempt
        if used or answer.startswith("__"):
            break
    if not used and not answer.startswith("__"):
        answer = "__NO_TOOLS__"
    out.update(arm=a.arm, model=a.model, seed=rep, task_id=task["id"], kind=task["kind"], answer=answer, tool_names=used,
               total_tokens=out["input_tokens"] + out["output_tokens"], n_tool_calls=len(used), accuracy=score_answer(answer, task["expected"]))
    return out


def run(a):
    system = constant(THEIRS / "tests/arms.py", "SYSTEM_PROMPT")
    deny = constant(THEIRS / "tests/runner.py", "DISALLOWED_TOOLS")
    turns = constant(THEIRS / "tests/runner.py", "MAX_TURNS")
    tasks = [json.loads(l) for l in open(THEIRS / "tests/tasks/tasks.jsonl")]
    if a.only:
        tasks = [t for t in tasks if t["id"] in a.only.split(",")]
    binary = Path(a.codefast).resolve()
    if a.arm == "codefast":
        t0 = time.perf_counter()
        subprocess.run([str(binary), "--root", str(SUPERSET), "status"], capture_output=True, text=True, check=True)
        print(f"index of superset ready after {time.perf_counter() - t0:.1f} s (0 when it was already there)")
    work = Path(tempfile.mkdtemp(prefix="acb-"))
    config = work / "mcp.json"
    config.write_text(json.dumps({"mcpServers": {a.arm: arm_server(a.arm, binary)}}))
    empty = work / "cwd"
    empty.mkdir()
    done = set()
    if a.out and Path(a.out).exists():
        done = {(r["task_id"], r["seed"]) for r in map(json.loads, open(a.out)) if r["arm"] == a.arm and r["model"] == a.model}
    todo = [(t, rep) for rep in range(a.reps) for t in tasks if (t["id"], rep) not in done]
    print(f"{len(todo)} runs to do, arm {a.arm}, model {MODELS[a.model]}, {a.jobs} at a time")
    # The published list of built-in tools to deny is that of the harness version it ran on. A
    # tool that has been added since is denied as well, so that the arm stays the only tool.
    for _ in range(3):
        first = one(a, *todo[0], system, deny, turns, str(empty), str(config))
        stray = [t for t in first["given"] if not t.startswith(f"mcp__{a.arm}__")]
        if not stray:
            break
        print(f"denied as well, being newer than the published list: {stray}")
        deny = deny + stray
    else:
        sys.exit(f"tools that are not the arm's still reach the session: {stray}")
    print(f"tools of a session: {first['given']}")

    def keep(r):
        print(f"  {r['task_id']:34s} run {r['seed']} acc={r['accuracy']:.2f} {r['total_tokens']:6d} tok {r['n_tool_calls']:2d} calls ${r['cost_usd']:.4f} {r['wall_s']:5.1f} s", flush=True)
        if a.out:
            r = {k: v for k, v in r.items() if k != "given"}
            with open(a.out, "a") as f:
                f.write(json.dumps(r) + "\n")

    keep(first)
    with ThreadPoolExecutor(a.jobs) as pool:
        for f in as_completed([pool.submit(one, a, *x, system, deny, turns, str(empty), str(config)) for x in todo[1:]]):
            keep(f.result())


def score(a):
    kinds = {json.loads(l)["id"]: json.loads(l)["kind"] for l in open(THEIRS / "tests/tasks/tasks.jsonl")}
    arms = collections.OrderedDict()
    for path in sorted((THEIRS / "data").glob(f"*__{a.model}.jsonl")):
        arms["published: " + path.name.split("__")[0].split("_", 1)[1]] = [json.loads(l) for l in open(path)]
    for path in a.rows:
        for r in map(json.loads, open(path)):
            if r["model"] == a.model:
                arms.setdefault("here: " + r["arm"], []).append(r)
    med = lambda xs: statistics.median(xs) if xs else 0
    print(f"model {MODELS[a.model]}: accuracy is the mean over runs, the other columns the median run")
    for regime, pick in (("the 20 lookups", lambda k: k not in HARD), ("the 6 hard tasks (which class, which overrides, who calls)", lambda k: k in HARD)):
        print(f"\n{regime}")
        print(f"{'arm':28s} {'runs':>4s} {'accuracy':>8s} {'answered':>8s} {'tokens':>7s} {'with cache':>10s} {'calls':>5s} {'$ a task':>8s} {'seconds':>7s}")
        for name, rows in arms.items():
            rs = [r for r in rows if pick(kinds[r["task_id"]])]
            if not rs:
                continue
            answered = sum(1 for r in rs if not r["answer"].startswith("__")) / len(rs)
            whole = [r["total_tokens"] + r.get("cache_read_tokens", 0) + r.get("cache_creation_tokens", 0) for r in rs]
            print(f"{name:28s} {len(rs):4d} {sum(r['accuracy'] for r in rs) / len(rs):8.2f} {answered:8.0%} {med([r['total_tokens'] for r in rs]):7.0f} {med(whole):10.0f} "
                  f"{med([r['n_tool_calls'] for r in rs]):5.0f} {med([r['cost_usd'] for r in rs]):8.3f} {med([r['wall_s'] for r in rs]):7.1f}")


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("fetch").set_defaults(fn=fetch)
    p = sub.add_parser("run")
    p.add_argument("--arm", default="codefast")
    p.add_argument("--model", default="haiku", choices=list(MODELS))
    p.add_argument("--reps", type=int, default=3)
    p.add_argument("--jobs", type=int, default=2)
    p.add_argument("--only", default="", help="task ids, comma separated")
    p.add_argument("--out", default="")
    p.add_argument("--timeout", type=int, default=600)
    p.add_argument("--codefast", default=os.environ.get("CODEFAST_BIN") or str(ROOT / "target/release/codefast"))
    p.add_argument("--claude", default="claude")
    p.add_argument("--config-dir", default=os.environ.get("CLAUDE_CONFIG_DIR", str(Path.home() / ".claude")))
    p.set_defaults(fn=run)
    p = sub.add_parser("score")
    p.add_argument("rows", nargs="*")
    p.add_argument("--model", default="haiku", choices=list(MODELS))
    p.set_defaults(fn=score)
    a = ap.parse_args()
    a.fn(a)


if __name__ == "__main__":
    main()
