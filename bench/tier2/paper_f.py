# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Kernel bug localisation under the protocol of the published comparison, as exactly as it can be run here.

"How Should Agents See Code?" (arXiv 2611.07286, code at
github.com/MelikaSepidband/How-Should-Agents-See-Code) gives Claude Code a LinuxFLBench report in
a checkout of the kernel version it was filed against and compares a session without a repository
map with sessions that have one. `live.py` has a family for the same task with a prompt of its
own. This script runs the paper's protocol instead: its prompt template, its system prompts, its
settings (Haiku 4.5 at low effort, the six built-in tools, 30 turns, a dollar a session), its
grading rule and its token count. The prompt template and the system prompts are read from a
clone of the paper's repository at run time and are not copied here.

Arms:
  baseline   no MCP server, the paper's neutral system prompt: its baseline
  codefast   codefast's MCP server, with a system prompt of the shape the paper gives its own
             tools (which tool to call first, that results carry code) and the paper's sentence
             that the tools must be used. This is the paper's treatment condition
  free       the same without the sentence that the tools must be used

The paper's 152 sampled reports are not published. `--set held` takes reports from the half of
LinuxFLBench the file ranking was never tuned on that no session has been run on, in the order
of a hash, and `--set dev` the ones `make_f_tasks.py --dev` takes.

usage: python3 -I bench/tier2/paper_f.py fetch
       python3 -I bench/tier2/paper_f.py run --tag T --set dev --n 20 --arms baseline,codefast --out ROWS.jsonl
       python3 -I bench/tier2/paper_f.py score ROWS.jsonl [MORE.jsonl]
"""
import argparse
import collections
import hashlib
import json
import os
import random
import re
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
THEIRS = ROOT / "bench/tools/hsasc/repo"
PINNED = "1512fa9"
DATA = ROOT / "bench/data/linuxflbench/upstream/dataset/LINUXFLBENCH_dataset.jsonl"
MODEL = "claude-haiku-4-5-20251001"
CAP = 6000
NOTE = "\n\n[bug description truncated at 6000 characters]"
SCRUB = ("CLAUDECODE", "CLAUDE_CODE_", "CLAUDE_PID", "CLAUDE_EFFORT", "ANTHROPIC_API_KEY", "CODEFAST_")
PUBLISHED = [
    ("published: no map (152 reports)", 0.572, 0.717, 1_008_163, 25.4),
    ("published: best map, Hybrid", 0.526, 0.829, 610_910, 10.0),
]

# What the paper's treatment arms are told about their tools, with codefast's tools in it: the
# shape and most of the words are those of `cg_steer` in the paper's conditions/prompts/rep_tools.py.
PLAYBOOK = """This server gives you an index of the code (definitions, references, calls and the text of every file), as MCP tools (prefixed mcp__). Reach for them BEFORE grep/find or reading files for ANY question about the code -- they are pre-computed structure you'd otherwise re-derive by reading, and their results carry the relevant code INLINE (treat shown source as already read -- do NOT re-open those files).

PRIMARY -- call FIRST for almost any question:
  - {primary}

NAVIGATION -- for targeted follow-up the first call didn't cover (their results also carry the relevant code inline):
  - search(queries) with an exact symbol name: its definition, its body and who uses it
  - refs(targets, mode): every use of a symbol, or its callers or callees
  - path(from, to): the call chain between two functions
  - grep(pattern): text search that says which function each hit is in

READ A DEFINITION (rarely needed):
  - show(targets): the body at 'file:line' or of an exact symbol, or a file's outline, for something NOT already shown inline. Most code arrives inline with the tools above -- do NOT loop show symbol-by-symbol; that's the slow path.

ANTI-PATTERNS: don't grep to re-verify a tool's result (it's from a full parse -- grep is slower and less accurate); don't loop show over many symbols when the first call (or a navigation tool) already returned their code inline; don't Read a whole file when the tools gave you the definition. Reach for raw Read/Grep only to confirm a specific detail the tools genuinely did not cover."""
PRIMARY = {
    # Free text given to codefast's `search` is answered the way the paper's `<map>_explore`
    # answers: the best files with the source of their relevant functions, in one capped call.
    "search": "search(queries): pass the bug report's key terms as ONE free-text query (several words, not a symbol name). Returns, in ONE capped call, the files that match best PLUS the VERBATIM SOURCE of the functions that make them candidates. It is Read-equivalent -- treat the shown source as already read, do NOT re-open those files. Usually the ONLY call you need.",
}


def is_dev(task_id):
    return int(hashlib.sha256(task_id.encode()).hexdigest(), 16) % 2 == 0


def fetch(_):
    if not THEIRS.is_dir():
        THEIRS.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(["git", "clone", "-q", "https://github.com/MelikaSepidband/How-Should-Agents-See-Code.git", str(THEIRS)], check=True)
    head = subprocess.run(["git", "-C", str(THEIRS), "rev-parse", "--short", "HEAD"], capture_output=True, text=True).stdout.strip()
    print(f"the paper's repository at {head}" + ("" if head.startswith(PINNED) else f" (these figures were made at {PINNED})"))


def tasks_of(which, n):
    rows = [json.loads(l) for l in open(DATA) if l.strip()]
    used = json.load(open(ROOT / "bench/tier2/tasks.json"))
    in_sessions = {x.split("-", 1)[1] for x in used["held_out"].get("F", []) + used.get("kernel_bug_reports_development", [])}
    if which == "dev":
        order = [x.split("-", 1)[1] for x in used["kernel_bug_reports_development"] if x.startswith("FD-")]
        by = {str(r["id"]): r for r in rows}
        rows = [by[i] for i in order if i in by]
    else:
        rows = [r for r in rows if not is_dev(str(r["id"])) and str(r["id"]) not in in_sessions]
        rows.sort(key=lambda r: hashlib.sha256(("pf:" + str(r["id"])).encode()).hexdigest())
    template = (THEIRS / "experiments/linuxflbench/prompt_template.md").read_text()
    out = []
    for r in rows:
        if len(out) == n:
            break
        tree = subprocess.run([str(ROOT / "bench/scripts/datasets/prepare_tree.sh"), "linuxflbench", str(r["id"])], capture_output=True, text=True)
        path = tree.stdout.strip().splitlines()[-1] if tree.returncode == 0 and tree.stdout.strip() else ""
        gold = [p.strip().lstrip("./") for p in r["paths"]]
        if not path or not all((Path(path) / g).is_file() for g in gold):
            continue
        d = r["description"] or ""
        prompt = template.replace("{{TITLE}}", r["title"] or "").replace("{{DESCRIPTION}}", d[:CAP] + NOTE if len(d) > CAP else d)
        out.append(dict(id=str(r["id"]), version=r["Kernel Version"], tree=path, prompt=prompt, gold=gold))
    return out


def system_prompt(arm, primary):
    neutral = (THEIRS / "conditions/prompts/neutral.md").read_text().rstrip("\n")
    if arm == "baseline":
        return neutral + "\n"
    parts = [neutral, PLAYBOOK.replace("{primary}", PRIMARY[primary])]
    if arm == "codefast":
        parts.append((THEIRS / "conditions/prompts/steer-strict.md").read_text().rstrip("\n"))
    return "\n\n".join(parts) + "\n"


def prediction(text):
    """The paper's rule: the last fenced JSON block that has `files`, else the last object in the
    text that has it."""
    fenced = []
    for m in re.finditer(r"```(?:json)?\s*\n(.*?)```", text, re.S):
        try:
            obj = json.loads(m.group(1).strip())
        except ValueError:
            continue
        if isinstance(obj, dict):
            fenced.append(obj)
    for obj in reversed(fenced):
        if "files" in obj:
            return obj
    dec, found = json.JSONDecoder(), None
    for i, ch in enumerate(text):
        if ch == "{":
            try:
                obj, _ = dec.raw_decode(text[i:])
            except ValueError:
                continue
            if isinstance(obj, dict) and "files" in obj:
                found = obj
    return found if found is not None else (fenced[-1] if fenced else None)


def norm(path, tree):
    p = str(path).strip().strip("`'\"").replace("\\", "/")
    if p.startswith(tree.rstrip("/") + "/"):
        p = p[len(tree.rstrip("/")) + 1:]
    while p.startswith("./"):
        p = p[2:]
    parts = p.lstrip("/").split("/")
    if len(parts) > 1 and re.match(r"^linux(-|@|$)", parts[0]):
        parts = parts[1:]
    return "/".join(parts)


def one(a, t, arm, rep, binary, run_dir):
    run_dir.mkdir(parents=True, exist_ok=True)
    tree = t["tree"]
    if not (Path(tree) / ".git").exists():
        # An exported tree inside this repository would be taken for a part of it.
        subprocess.run(["git", "init", "-q", "."], cwd=tree, check=True)
    sysfile = run_dir / "system_prompt.md"
    sysfile.write_text(system_prompt(arm, a.primary))
    cmd = [a.claude, "-p", t["prompt"], "--output-format", "stream-json", "--verbose", "--strict-mcp-config"]
    if arm != "baseline":
        index = ROOT / "target/bench-index" / ("lfl-" + Path(tree).name)
        cfg = run_dir / "mcp.json"
        cfg.write_text(json.dumps({"mcpServers": {"codefast": {"type": "stdio", "command": str(binary), "args": ["mcp", "--root", tree],
                                                                 "env": {"CODEFAST_INDEX_DIR": str(index), "CODEFAST_ROOT": tree, "CODEFAST_THREADS": "2"}}}}))
        cmd += ["--mcp-config", str(cfg)]
    cmd += ["--model", MODEL, "--effort", "low", "--tools", "Read,Edit,Write,Bash,Grep,Glob", "--permission-mode", "bypassPermissions",
            "--max-turns", "30", "--max-budget-usd", "1", "--disallowedTools", "WebFetch,WebSearch", "--append-system-prompt-file", str(sysfile),
            "--no-session-persistence", "--setting-sources", "project", "--no-chrome"]
    env = {k: v for k, v in os.environ.items() if not k.startswith(SCRUB)}
    env.update(CLAUDE_CONFIG_DIR=a.config_dir, ENABLE_CLAUDEAI_MCP_SERVERS="false")
    t0 = time.time()
    try:
        out = subprocess.run(cmd, cwd=tree, env=env, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=900).stdout
    except subprocess.TimeoutExpired:
        out = ""
    (run_dir / "run.jsonl").write_text(out)
    row: dict = dict(id=t["id"], arm=arm, rep=rep, tag=a.tag, version=t["version"], seconds=round(time.time() - t0), answer="", subtype="", tokens=0, cost=0.0, turns=0, mcp=0, native=0, tools={})
    for line in out.splitlines():
        try:
            ev = json.loads(line)
        except ValueError:
            continue
        if ev.get("type") == "assistant":
            for b in ev.get("message", {}).get("content", []):
                if b.get("type") == "tool_use":
                    name = b.get("name", "?")
                    row["tools"][name] = row["tools"].get(name, 0) + 1
                    row["mcp" if name.startswith("mcp__") else "native"] += 1
        elif ev.get("type") == "result":
            u = ev.get("usage") or {}
            row.update(answer=str(ev.get("result") or ""), subtype=str(ev.get("subtype") or ""), turns=int(ev.get("num_turns") or 0), cost=float(ev.get("total_cost_usd") or 0.0),
                       tokens=sum(int(u.get(k) or 0) for k in ("input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens", "output_tokens")))
    pred = prediction(row["answer"])
    files = [norm(p, tree) for p in pred["files"] if str(p).strip()] if pred and isinstance(pred.get("files"), list) else []
    row["parsed"] = bool(pred and isinstance(pred.get("files"), list))
    row["files"] = files[:10]
    row["rank"] = next((i + 1 for i, f in enumerate(files[:10]) if f in t["gold"]), None)
    del row["answer"]
    return row


def run(a):
    tasks = tasks_of(a.set, a.n)
    if a.ids:
        tasks = [t for t in tasks_of(a.set, 10**6) if t["id"] in a.ids.split(",")]
    arms = a.arms.split(",")
    binary = Path(a.codefast).resolve()
    if any(x != "baseline" for x in arms):
        for tree in sorted({t["tree"] for t in tasks}):
            env = dict(os.environ, CODEFAST_INDEX_DIR=str(ROOT / "target/bench-index" / ("lfl-" + Path(tree).name)), CODEFAST_ROOT=tree)
            subprocess.run([str(binary), "--root", tree, "status"], capture_output=True, text=True, env=env)
    done = set()
    if a.out and Path(a.out).exists():
        done = {(r["id"], r["arm"], r["rep"]) for r in map(json.loads, open(a.out)) if r["tag"] == a.tag}
    todo = [(t, arm, rep) for rep in range(a.reps) for t in tasks for arm in arms if (t["id"], arm, rep) not in done]
    print(f"{len(tasks)} reports x {len(arms)} arms x {a.reps}, {len(todo)} sessions to run, {a.jobs} at a time", flush=True)
    with ThreadPoolExecutor(a.jobs) as pool:
        futs = [pool.submit(one, a, t, arm, rep, binary, ROOT / "target/live" / a.tag / t["id"] / (arm + (f".{rep}" if rep else ""))) for t, arm, rep in todo]
        for f in as_completed(futs):
            r = f.result()
            print(f"  {r['id']:8s} {r['arm']:9s} rank={str(r['rank']):4s} files={len(r['files']):2d} turns={r['turns']:2d} mcp={r['mcp']:2d} native={r['native']:2d} tokens={r['tokens']:9,d} ${r['cost']:.3f} {r['seconds']:3d}s {r['subtype']}", flush=True)
            if a.out:
                with open(a.out, "a") as fh:
                    fh.write(json.dumps(r) + "\n")


def score(a):
    by = collections.defaultdict(lambda: collections.defaultdict(list))
    for path in a.rows:
        for r in map(json.loads, open(path)):
            by[r["arm"] if not a.by_tag else f"{r['arm']} ({r['tag']})"][r["id"]].append(r)
    arms = list(by)
    ids = sorted(set.intersection(*(set(by[x]) for x in arms)))
    mean = lambda xs: sum(xs) / len(xs) if xs else 0.0
    hit = lambda r, k: 1.0 if r.get("rank") and r["rank"] <= k else 0.0
    print(f"{len(ids)} reports that every arm ran")
    print(f"{'arm':36s} {'file first':>10s} {'in the ten':>10s} {'tokens':>10s} {'calls':>6s} {'of them MCP':>11s} {'files':>6s} {'no answer':>9s} {'cost':>6s}")
    for label, r1, r10, tok, calls in PUBLISHED:
        print(f"{label:36s} {r1:10.3f} {r10:10.3f} {tok:10,d} {calls:6.1f}")
    base = None
    for arm in arms:
        f = lambda fn: mean([mean([fn(r) for r in by[arm][t]]) for t in ids])
        line = (f(lambda r: hit(r, 1)), f(lambda r: hit(r, 10)), f(lambda r: r["tokens"]), f(lambda r: r["mcp"] + r["native"]), f(lambda r: r["mcp"]), f(lambda r: len(r["files"])),
                sum(mean([0.0 if r["parsed"] else 1.0 for r in by[arm][t]]) for t in ids), f(lambda r: r["cost"]))
        print(f"{arm:36s} {line[0]:10.3f} {line[1]:10.3f} {line[2]:10,.0f} {line[3]:6.1f} {line[4]:11.1f} {line[5]:6.1f} {line[6]:9.1f} {line[7]:6.3f}")
        if base is None:
            base = (arm, line)
            continue
        b = base[1]
        win = sum(1 for t in ids if mean([hit(r, 10) for r in by[arm][t]]) > mean([hit(r, 10) for r in by[base[0]][t]]))
        loss = sum(1 for t in ids if mean([hit(r, 10) for r in by[arm][t]]) < mean([hit(r, 10) for r in by[base[0]][t]]))
        random.seed(3)
        d10, dtok = [], []
        for _ in range(10000):
            s = [random.choice(ids) for _ in ids]
            g = lambda x, fn: mean([mean([fn(r) for r in by[x][t]]) for t in s])
            d10.append(g(arm, lambda r: hit(r, 10)) - g(base[0], lambda r: hit(r, 10)))
            dtok.append(g(arm, lambda r: r["tokens"]) / g(base[0], lambda r: r["tokens"]) - 1)
        d10.sort()
        dtok.sort()
        print(f"{'  against ' + base[0]:36s} {line[0] - b[0]:+10.3f} {line[1] - b[1]:+10.3f} {line[2] / b[2] - 1:+10.0%}   into the ten for {win}, out of it for {loss}; in the ten {d10[250]:+.3f} to {d10[9750]:+.3f}, tokens {dtok[250]:+.0%} to {dtok[9750]:+.0%}")


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("fetch").set_defaults(fn=fetch)
    p = sub.add_parser("run")
    p.add_argument("--tag", required=True)
    p.add_argument("--set", default="dev", choices=["dev", "held"])
    p.add_argument("--n", type=int, default=20)
    p.add_argument("--ids", default="")
    p.add_argument("--arms", default="baseline,codefast")
    p.add_argument("--primary", default="search", choices=list(PRIMARY), help="the tool the system prompt names as the first call")
    p.add_argument("--reps", type=int, default=1)
    p.add_argument("--jobs", type=int, default=2)
    p.add_argument("--out", default="")
    p.add_argument("--codefast", default=os.environ.get("CODEFAST_BIN") or str(ROOT / "target/release/codefast"))
    p.add_argument("--claude", default="claude")
    p.add_argument("--config-dir", default=os.environ.get("CLAUDE_CONFIG_DIR", str(Path.home() / ".claude")))
    p.set_defaults(fn=run)
    p = sub.add_parser("score")
    p.add_argument("rows", nargs="+")
    p.add_argument("--by-tag", action="store_true", help="keep the batches apart: an arm of two builds is two rows")
    p.set_defaults(fn=score)
    a = ap.parse_args()
    a.fn(a)


if __name__ == "__main__":
    main()
