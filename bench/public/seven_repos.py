"""The seven-repository protocol that CodeGraph publishes, run with codefast as the arm.

Seven open-source repositories cloned at depth 1, one architecture question each, the session
run headless (`claude -p`) with and without the tool, several runs per arm and the median
reported: tokens processed (input with cache reads and writes, plus output, summed over the
requests of the session, sub-agents included), the harness's own dollar figure, every tool
call, and wall-clock time. The repositories and the questions are theirs, word for word
(https://github.com/colbymchenry/codegraph, README, measured 2026-08-05 on Claude Opus 4.8:
62% fewer tokens, 44% cheaper, 88% fewer tool calls, 53% faster).

What differs here, and is said wherever the numbers are shown: the model is the one named
with `--model` (theirs is no longer current), the arm with the tool is the codefast plugin as
people install it (hooks, skill, command and MCP server), and the answers get a check that
theirs did not have: which of the files an answer names exist in the repository.

usage:
  seven_repos.py clone                      fetch the repositories that are missing (depth 1)
  seven_repos.py run --tag T --model opus [--reps 4] [--jobs 2] [--repos a,b] [--arms control,plugin]
                     [--limit-stop 0.8] [--resume] [--openrouter --max-spend 15]
  seven_repos.py score --tag T [--out FILE.jsonl]
"""
import argparse
import concurrent.futures
import json
import os
import re
import shutil
import statistics
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tier2"))
import live  # noqa: E402

ROOT = live.ROOT
REPOS = ROOT / "bench/repos"
INDEX = ROOT / "target/bench-index"

# name, where it is cloned from, and the question as published.
TASKS = [
    ("vscode", "microsoft/vscode", "How does the extension host communicate with the main process?"),
    ("excalidraw", "excalidraw/excalidraw", "How does Excalidraw render and update canvas elements?"),
    ("django", "django/django", "How does Django's ORM build and execute a query from a QuerySet?"),
    ("tokio", "tokio-rs/tokio", "How does tokio schedule and run async tasks on its runtime?"),
    ("okhttp", "square/okhttp", "How does OkHttp process a request through its interceptor chain?"),
    ("gin", "gin-gonic/gin", "How does gin route requests through its middleware chain?"),
    ("alamofire", "Alamofire/Alamofire", "How does Alamofire build, send, and validate a request?"),
]
# What they published, as (seconds, tool calls, tokens, dollars) with and without their tool.
PUBLISHED = {
    "vscode": ((58, 2, 155_000, 0.53), (130, 28, 670_000, 1.80)),
    "excalidraw": ((45, 2, 156_000, 0.54), (162, 43, 991_000, 2.43)),
    "django": ((54, 3, 183_000, 0.55), (83, 14, 309_000, 0.63)),
    "tokio": ((63, 3, 201_000, 0.66), (163, 29, 573_000, 1.83)),
    "okhttp": ((33, 1, 107_000, 0.39), (58, 6, 230_000, 0.50)),
    "gin": ((28, 1, 87_000, 0.31), (46, 7, 180_000, 0.31)),
    "alamofire": ((54, 4, 209_000, 0.54), (142, 33, 505_000, 1.27)),
}
BUDGET = {"haiku": 2.5, "sonnet": 8.0, "opus": 20.0, "fable": 40.0}


def clone(_a):
    for name, origin, _ in TASKS:
        dst = REPOS / name
        if dst.exists():
            print(f"{name}: there ({subprocess.run(['git', '-C', str(dst), 'log', '-1', '--format=%h %cs'], capture_output=True, text=True).stdout.strip()})")
            continue
        rc = subprocess.run(["git", "clone", "-q", "--depth", "1", f"https://github.com/{origin}", str(dst)]).returncode
        print(f"{name}: cloned from {origin}" if rc == 0 else f"{name}: clone failed ({rc})")
    # What a repository brings for a session of its own: said, because both arms get it.
    for name, _, _ in TASKS:
        dst = REPOS / name
        found = [p for p in (".claude", ".mcp.json", "CLAUDE.md", "AGENTS.md") if (dst / p).exists()]
        if found:
            print(f"  {name} carries {', '.join(found)}")


def one(row, a, binary, plug):
    run = Path(row["run"])
    env = {k: v for k, v in os.environ.items() if k not in live.SCRUB}
    env.update(CLAUDE_CONFIG_DIR=a.config_dir, CODEFAST_BIN=str(binary), PYTHONDONTWRITEBYTECODE="1", ENABLE_CLAUDEAI_MCP_SERVERS="false")
    key = os.environ.get("OPENROUTER_API_KEY", "") if a.openrouter else ""
    if a.openrouter:
        # The same harness and the same models, paid by the token through OpenRouter. Its
        # prompt cache lives five minutes, so a cache write costs 1.25 of input there, where a
        # subscription's main thread writes to the one-hour cache at 2.
        env.update(ANTHROPIC_BASE_URL="https://openrouter.ai/api", ANTHROPIC_AUTH_TOKEN=key, ANTHROPIC_API_KEY="", DISABLE_AUTOUPDATER="1")
        env.update({f"ANTHROPIC_DEFAULT_{m.upper()}_MODEL": f"anthropic/claude-{m}-5.5" for m in ("opus", "sonnet", "haiku")})
    for k in ("CODEFAST_HOOKS", "CODEFAST_INDEX_DIR", "CODEFAST_ROOT"):
        env.pop(k, None)
    cmd = [
        a.claude, "-p", "--model", a.model, "--output-format", "stream-json", "--verbose", "--include-hook-events", "--setting-sources", "project",
        "--dangerously-skip-permissions", "--disallowedTools", live.DENY, "--max-turns", "200", "--max-budget-usd", str(BUDGET.get(a.model, 20.0)), "--no-chrome",
        "--no-session-persistence",
    ]
    if a.effort:
        cmd += ["--effort", a.effort]
    if row["arm"] == "plugin":
        # The index lies outside the repository, so that the other arm never finds one there.
        env["CODEFAST_INDEX_DIR"] = row["index"]
        cmd += ["--plugin-dir", str(plug)]
    else:
        cmd += ["--strict-mcp-config"]
    t0 = time.time()
    with open(run / "run.jsonl", "w") as out, open(run / "err.txt", "w") as err:
        try:
            rc = subprocess.run(cmd, cwd=row["repo"], env=env, stdin=open(run / "prompt.txt"), stdout=out, stderr=err, timeout=a.timeout).returncode
        except subprocess.TimeoutExpired:
            rc = 124
    seconds = round(time.time() - t0, 1)
    if key:
        # The key is in no record of a run.
        for name in ("run.jsonl", "err.txt"):
            text = (run / name).read_text(errors="replace")
            if key in text:
                (run / name).write_text(text.replace(key, "<key>"))
    res = dict(row, exit=rc, seconds=seconds)
    json.dump(res, open(run / "outcome.json", "w"), indent=1)
    return res


def run(a):
    assert a.config_dir, "set CLAUDE_CONFIG_DIR: which login the sessions use is never left to chance"
    assert not a.openrouter or os.environ.get("OPENROUTER_API_KEY"), "--openrouter needs OPENROUTER_API_KEY in the environment"
    base = live.LIVE / a.tag
    base.mkdir(parents=True, exist_ok=True)
    binary = base / "codefast"
    if not binary.exists():
        shutil.copy2(a.codefast, binary)
    plug = live.plugin_dir(a.tag)
    wanted = set(a.repos.split(",")) if a.repos else None
    tasks = [t for t in TASKS if not wanted or t[0] in wanted]
    arms = a.arms.split(",")
    for name, _, _ in tasks:
        assert (REPOS / name).is_dir(), f"{name} is not cloned: run `seven_repos.py clone`"
        if "plugin" in arms:
            # Written by this batch's binary before any session, as an installed plugin would
            # have it from the first session on.
            index = INDEX / f"public-{name}"
            t0 = time.time()
            e = dict(os.environ, CODEFAST_INDEX_DIR=str(index))
            subprocess.run([str(binary), "--root", str(REPOS / name), "index"], capture_output=True, text=True, env=e)
            print(f"  index of {name}: {time.time() - t0:.1f} s", flush=True)
    todo = []
    # Repetitions are interleaved and the arms alternate, so that a change in the account's
    # speed or in the load of the machine falls on both arms alike.
    for rep in range(a.reps):
        for name, origin, question in tasks:
            for arm in arms if rep % 2 == 0 else arms[::-1]:
                d = base / name / (arm if rep == 0 else f"{arm}.{rep}")
                if a.resume and (d / "outcome.json").exists():
                    continue
                if d.exists():
                    shutil.rmtree(d)
                d.mkdir(parents=True)
                (d / "prompt.txt").write_text(question + "\n")
                todo.append(dict(id=name, origin=origin, arm=arm, rep=rep, repo=str(REPOS / name), run=str(d), model=a.model, index=str(INDEX / f"public-{name}"), question=question))
    print(f"{len(tasks)} repositories x {len(arms)} arms x {a.reps} repetitions, {len(todo)} runs to do, model {a.model}, {a.jobs} at a time", flush=True)
    stop = False
    spent = 0.0

    def go(row):
        return None if stop else one(row, a, binary, plug)

    with concurrent.futures.ThreadPoolExecutor(a.jobs) as ex:
        for res in ex.map(go, todo):
            if res is None:
                continue
            r = live.parse(Path(res["run"]) / "run.jsonl")
            lim = r["limits"]
            worst = max([v for k, v in lim.items() if k != "status" and isinstance(v, (int, float))] or [0])
            print(f"  {res['id']:11} {res['arm']:8} exit={res['exit']} req={r['requests']:3d} tools={sum(r['tools'].values()):3d} ${r['cost']:.3f} {res['seconds']:.0f}s limits={lim}", flush=True)
            if lim.get("status") not in (None, "allowed") or worst >= a.limit_stop:
                print(f"  STOPPING: account usage window at {worst:.2f} (status {lim.get('status')})", flush=True)
                stop = True
            spent += r["cost"] or 0
            if a.max_spend and spent >= a.max_spend and not stop:
                print(f"  STOPPING: the sessions of this batch have cost ${spent:.2f}, and --max-spend is ${a.max_spend:.2f}", flush=True)
                stop = True


def named_files(text, repo):
    """The files an answer names that exist in the repository: a path with a directory in it,
    or a bare file name that exactly one file of the repository has."""
    files = subprocess.run(["git", "-C", repo, "ls-files"], capture_output=True, text=True).stdout.split("\n")
    known = set(files)
    by_name = {}
    for f in files:
        by_name.setdefault(f.rsplit("/", 1)[-1], []).append(f)
    said = set(re.findall(r"[\w@.+-]+(?:/[\w@.+-]+)*\.[A-Za-z]{1,6}\b", text))
    real, unknown = set(), set()
    for s in said:
        s = s.lstrip("./")
        if s in known:
            real.add(s)
        elif "/" in s:
            hits = [f for f in files if f.endswith("/" + s)]
            (real if hits else unknown).add(hits[0] if hits else s)
        elif len(by_name.get(s, [])) == 1:
            real.add(by_name[s][0])
        elif s not in by_name and re.search(r"\.(ts|tsx|js|py|rs|go|kt|java|swift|c|h|md)$", s):
            unknown.add(s)
    return sorted(real), sorted(unknown)


def rows_of(tag):
    rows = []
    for out in sorted((live.LIVE / tag).glob("*/*/outcome.json")):
        res = json.load(open(out))
        r = live.parse(out.parent / "run.jsonl")
        u = r["usage"] or dict(input=0, cache_read=0, cache_write=0, output=0)
        text = live.final_text(str(out.parent))
        real, unknown = named_files(text, res["repo"])
        rows.append(dict(
            tag=tag, date=time.strftime("%Y-%m-%d", time.localtime(out.stat().st_mtime)), id=res["id"], arm=res["arm"], rep=res["rep"], model=res["model"], family="X",
            seconds=res["seconds"], exit=res["exit"], final=r["final"], requests=r["requests"], sub_requests=r["sub_requests"], tool_calls=sum(r["tools"].values()), tools=r["tools"],
            tokens=u["input"] + u["cache_read"] + u["cache_write"] + u["output"], cost=r["cost"], **u, cf_cli=r["cf_cli"], cf_mcp=r["cf_mcp"], hooks=r["hooks"], hook_bytes=r["hook_bytes"],
            mcp=r["init"].get("mcp"), answer_chars=len(text), files_named=len(real), files_named_missing=len(unknown),
            violations=(["the control used the tool"] if res["arm"] == "control" and (r["cf_cli"] or r["cf_mcp"]) else []) + (["ended early: " + str(r["final"])] if r["final"] != "success" else []),
        ))
    return rows


def score(a):
    rows = rows_of(a.tag)
    if a.out:
        with open(a.out, "w") as f:
            for r in rows:
                f.write(json.dumps(r) + "\n")
    med = statistics.median
    by = {}
    for r in rows:
        by.setdefault((r["id"], r["arm"]), []).append(r)
    model = rows[0]["model"] if rows else "?"
    print(f"{a.tag}, model {model}: the median of the runs of each arm (control, with codefast), and what was published for the same question\n")
    print(f"{'repository':11} {'runs':>5} | {'seconds':>11} {'tool calls':>11} {'tokens':>15} {'dollars':>13} | {'time':>5} {'calls':>6} {'tokens':>7} {'cost':>6} | {'published: time calls tokens cost':>34} | files named (missing)")
    ratios = dict(seconds=[], tool_calls=[], tokens=[], cost=[])
    pub = dict(seconds=[], tool_calls=[], tokens=[], cost=[])
    for name, _, _ in TASKS:
        c, p = by.get((name, "control")), by.get((name, "plugin"))
        if not c or not p:
            continue
        m = {k: (med(x[k] for x in c), med(x[k] for x in p)) for k in ratios}
        for k in ratios:
            ratios[k].append(m[k][1] / m[k][0] if m[k][0] else 1.0)
        w, wo = PUBLISHED[name]
        for k, i in (("seconds", 0), ("tool_calls", 1), ("tokens", 2), ("cost", 3)):
            pub[k].append(w[i] / wo[i])
        files = f"{med(x['files_named'] for x in c):.0f} ({med(x['files_named_missing'] for x in c):.0f}), {med(x['files_named'] for x in p):.0f} ({med(x['files_named_missing'] for x in p):.0f})"
        print(
            f"{name:11} {len(c):2},{len(p):2} | {m['seconds'][0]:5.0f},{m['seconds'][1]:5.0f} {m['tool_calls'][0]:5.0f},{m['tool_calls'][1]:5.0f} {m['tokens'][0] / 1000:6.0f}k,{m['tokens'][1] / 1000:6.0f}k "
            f"{m['cost'][0]:6.2f},{m['cost'][1]:6.2f} | {ratios['seconds'][-1]:5.2f} {ratios['tool_calls'][-1]:6.2f} {ratios['tokens'][-1]:7.2f} {ratios['cost'][-1]:6.2f} | "
            f"{pub['seconds'][-1]:10.2f} {pub['tool_calls'][-1]:6.2f} {pub['tokens'][-1]:6.2f} {pub['cost'][-1]:6.2f} | {files}"
        )
    if ratios["cost"]:
        mean = statistics.mean
        print(
            f"\nwith codefast over control, mean of the repositories: time {mean(ratios['seconds']):.2f}, tool calls {mean(ratios['tool_calls']):.2f}, tokens {mean(ratios['tokens']):.2f}, cost {mean(ratios['cost']):.2f}"
            f"\npublished for the other tool (Opus 4.8):              time {mean(pub['seconds']):.2f}, tool calls {mean(pub['tool_calls']):.2f}, tokens {mean(pub['tokens']):.2f}, cost {mean(pub['cost']):.2f}"
        )
    bad = [(r["id"], r["arm"], r["rep"], v) for r in rows for v in r["violations"]]
    for b in bad:
        print("  violation:", b)
    others = sorted({m for r in rows if r["arm"] == "plugin" for m in (r["mcp"] or []) if "codefast" not in m})
    if others:
        print("  other MCP servers in the arm with the plugin:", others)


def main():
    p = argparse.ArgumentParser()
    sub = p.add_subparsers(dest="cmd", required=True)
    sub.add_parser("clone")
    r = sub.add_parser("run")
    r.add_argument("--tag", required=True)
    r.add_argument("--model", default="opus")
    r.add_argument("--arms", default="control,plugin")
    r.add_argument("--reps", type=int, default=4)
    r.add_argument("--jobs", type=int, default=2)
    r.add_argument("--repos", default="")
    r.add_argument("--effort", default="")
    r.add_argument("--timeout", type=int, default=1800)
    r.add_argument("--resume", action="store_true")
    r.add_argument("--limit-stop", type=float, default=0.8, help="stop the batch when an account usage window is this full")
    r.add_argument("--openrouter", action="store_true", help="run the sessions through OpenRouter with OPENROUTER_API_KEY, and --config-dir a directory with no login in it")
    r.add_argument("--max-spend", type=float, default=0.0, help="stop the batch when its sessions have cost this many dollars (0: no bound)")
    r.add_argument("--claude", default=os.environ.get("CLAUDE_BIN", str(Path.home() / ".local/bin/claude")))
    r.add_argument("--config-dir", default=os.environ.get("CLAUDE_CONFIG_DIR", ""))
    r.add_argument("--codefast", default=os.environ.get("CODEFAST_BIN", str(ROOT / "target/release/codefast")))
    s = sub.add_parser("score")
    s.add_argument("--tag", required=True)
    s.add_argument("--out", default="")
    a = p.parse_args()
    {"clone": clone, "run": run, "score": score}[a.cmd](a)


if __name__ == "__main__":
    main()
