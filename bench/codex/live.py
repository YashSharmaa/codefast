# /// script
# requires-python = ">=3.12"
# dependencies = ["tiktoken"]
# ///
"""The plugin inside Codex, with a model behind it.

The tasks, the prompts and the checks are those of `bench/tier2/live.py`, which runs them in
Claude Code. Here each one is run by `codex exec`, once in a home that has nothing of codefast
in it and once in a home where `codefast setup codex --hooks --apply` was run: the MCP server,
the hooks, the skill, and the command on PATH. Nothing else differs between the two.

The model is reached through OpenRouter. Codex reads the key from the environment by name
(`OPENROUTER_API_KEY`). It is never written to a file, and the shell commands of a session do
not get it (Codex leaves variables named like keys out of what it gives them).

Only the families that change nothing are run (questions about callers, callees, errors, the
kernel, and where a reported bug is), in Codex's read-only sandbox. Your own `~/.codex` is not
read: every session has a home of its own under `target/live/<tag>`.

What is counted is what Codex reports for a session: the tokens of all its requests (those the
provider served from its cache apart), and the tokens it wrote. Dollars are those tokens at the
provider's list prices, fetched when the batch starts and kept with the results.

usage (with uv, which brings what the check of the kernel questions imports):
  uv run bench/codex/live.py run --tag T --held N,C,E,K2 [--model openai/gpt-6.1-sol]
                                 [--jobs 2] [--codefast target/release/codefast]
  uv run bench/codex/live.py verify --tag T     check the finished sessions again
  uv run bench/codex/live.py score --tag T [--out bench/results/codex-live.jsonl]
"""
import argparse
import collections
import concurrent.futures
import importlib.util
import json
import os
import random
import re
import shutil
import subprocess
import sys
import time
import urllib.request
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
PROVIDER = "https://openrouter.ai/api/v1"
READ_ONLY = ("K", "N", "L", "E", "C", "F")


def tier2():
    """`bench/tier2/live.py` as a module: its tasks, its prompts and its checks."""
    spec = importlib.util.spec_from_file_location("tier2_live", ROOT / "bench/tier2/live.py")
    assert spec and spec.loader
    mod = importlib.util.module_from_spec(spec)
    argv, sys.argv = sys.argv, [sys.argv[0]]
    try:
        spec.loader.exec_module(mod)
    finally:
        sys.argv = argv
    return mod


def prices(model, keep):
    """List prices of the model in dollars a token: fresh input, input served from the
    provider's cache, output."""
    if keep.exists():
        return json.load(open(keep))
    req = urllib.request.Request(PROVIDER + "/models", headers={"User-Agent": "bench-client"})
    data = json.load(urllib.request.urlopen(req, timeout=60))["data"]
    p = next(m["pricing"] for m in data if m["id"] == model)
    out = dict(model=model, input=float(p["prompt"]), cached=float(p.get("input_cache_read") or p["prompt"]), output=float(p["completion"]), fetched=time.strftime("%Y-%m-%d"))
    json.dump(out, open(keep, "w"))
    return out


def make_home(home, model, binary, plugin):
    """A Codex home with the provider in it and, for the plugin arm, what `codefast setup codex`
    writes. No session runs in this one: each gets a copy."""
    if home.exists():
        shutil.rmtree(home)
    (home / ".codex").mkdir(parents=True)
    (home / ".codex/config.toml").write_text(
        f'model = "{model}"\nmodel_provider = "openrouter"\n\n[model_providers.openrouter]\nname = "OpenRouter"\n'
        f'base_url = "{PROVIDER}"\nenv_key = "OPENROUTER_API_KEY"\nwire_api = "responses"\n'
    )
    if plugin:
        env = {"HOME": str(home), "CODEX_HOME": str(home / ".codex"), "PATH": f"{binary.parent}:/usr/bin:/bin", "LANG": "en_US.UTF-8"}
        scratch = home / "setup-cwd"
        scratch.mkdir()
        p = subprocess.run([str(binary), "setup", "codex", "--hooks", "--apply"], cwd=scratch, env=env, capture_output=True, text=True)
        if p.returncode:
            sys.exit(f"codefast setup codex: {p.stdout}{p.stderr}")


def session(row, a, mod, binary, codex):
    run = Path(row["run"])
    plugin = mod.ARMS[row["arm"]]["plugin"]
    home = run / "home"
    shutil.copytree(ROOT / "target/live" / a.tag / f"home-{row['arm']}", home)
    # What the session's own tools see of codefast: the index of the shared tree it works in.
    extra = {}
    if plugin:
        if row["family"] == "K":
            extra["CODEFAST_INDEX_DIR"] = str(mod.LINUX_INDEX)
        elif row["family"] == "F":
            extra["CODEFAST_INDEX_DIR"] = row["index"]
            extra["CODEFAST_ROOT"] = row["repo"]
        else:
            extra["CODEFAST_INDEX_DIR"] = row.get("index") or str(mod.SYFT_INDEX)
        # Codex starts an MCP server with a short list of variables, so these are named for it.
        with open(home / ".codex/config.toml", "a") as f:
            f.write("\n[mcp_servers.codefast.env]\n" + "".join(f'{k} = "{v}"\n' for k, v in extra.items()))
    path = [p for p in os.environ.get("PATH", "/usr/bin:/bin").split(":") if p and not (Path(p) / "codefast").exists()]
    if plugin:
        path.insert(0, str(binary.parent))
    (run / "tmp").mkdir(exist_ok=True)
    env = {"HOME": str(home), "CODEX_HOME": str(home / ".codex"), "PATH": ":".join(path), "TMPDIR": str(run / "tmp"), "TERM": "dumb", "LANG": "en_US.UTF-8",
           "OPENROUTER_API_KEY": os.environ["OPENROUTER_API_KEY"], **extra}
    cmd = [codex, "exec", "--json", "--skip-git-repo-check", "--dangerously-bypass-hook-trust", "-s", "read-only", "-C", row["repo"], (run / "prompt.txt").read_text()]
    t0 = time.time()
    try:
        p = subprocess.run(cmd, env=env, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=a.timeout)
        rc, out, err = p.returncode, p.stdout, p.stderr
    except subprocess.TimeoutExpired as e:
        rc, out, err = 124, (e.stdout or b"").decode(errors="replace") if isinstance(e.stdout, bytes) else (e.stdout or ""), "timeout"
    (run / "codex.jsonl").write_text(out)
    (run / "err.txt").write_text(err[-20000:])
    shutil.rmtree(home, ignore_errors=True)
    shutil.rmtree(run / "tmp", ignore_errors=True)
    r = parse(run / "codex.jsonl")
    # The checks of the other harness read the answer from its event stream: one line of it.
    (run / "run.jsonl").write_text(json.dumps({"type": "result", "subtype": "success" if rc == 0 else "error", "result": r["final"]}) + "\n")
    try:
        ok, clean = mod.verify(row)
    except Exception as e:  # the session is on disk and can be checked again
        print(f"  check failed for {row['id']} {row['arm']}: {type(e).__name__}: {e}", flush=True)
        ok, clean = False, False
    res = dict(row, exit=rc, seconds=round(time.time() - t0), passed=ok, clean=clean, **{k: v for k, v in r.items() if k != "final"})
    json.dump(res, open(run / "outcome.json", "w"), indent=1)
    return res


def parse(path):
    """What Codex says a session did."""
    r: dict[str, Any] = dict(input=0, cached=0, output=0, reasoning=0, turns=0, items={}, cf_mcp=0, cf_cli=0, tool_calls=0, final="", errors=[])
    for line in open(path, errors="replace"):
        try:
            e = json.loads(line)
        except ValueError:
            continue
        t, item = e.get("type"), e.get("item") or {}
        if t == "turn.completed":
            u = e.get("usage") or {}
            r["turns"] += 1
            r["input"] += int(u.get("input_tokens") or 0)
            r["cached"] += int(u.get("cached_input_tokens") or 0)
            r["output"] += int(u.get("output_tokens") or 0)
            r["reasoning"] += int(u.get("reasoning_output_tokens") or 0)
        elif t in ("error", "turn.failed"):
            r["errors"].append(json.dumps(e)[:300])
        elif t == "item.completed":
            kind = item.get("type", "?")
            if kind == "agent_message":
                r["final"] = item.get("text") or ""
                continue
            if kind == "error":
                if "bypass-hook-trust" not in (item.get("message") or ""):
                    r["errors"].append((item.get("message") or "")[:300])
                continue
            r["items"][kind] = r["items"].get(kind, 0) + 1
            if kind == "reasoning":
                continue
            r["tool_calls"] += 1
            if kind == "mcp_tool_call" and item.get("server") == "codefast":
                r["cf_mcp"] += 1
            if kind == "command_execution" and re.search(r"(^|[\s;&|(`$'\"])(\S*/)?codefast(\s|$)", item.get("command") or ""):
                r["cf_cli"] += 1
            # A tool run from the model's script (how this Codex gives some models their tools).
            blob = json.dumps(item)
            if kind not in ("mcp_tool_call", "command_execution") and "mcp__codefast__" in blob:
                r["cf_mcp"] += 1
    return r


def cmd_run(a):
    if not os.environ.get("OPENROUTER_API_KEY"):
        sys.exit("OPENROUTER_API_KEY is not set")
    codex = shutil.which(a.codex) or sys.exit(f"{a.codex} is not on PATH")
    mod = tier2()
    base = ROOT / "target/live" / a.tag
    base.mkdir(parents=True, exist_ok=True)
    (base / "bin").mkdir(exist_ok=True)
    binary = base / "bin/codefast"
    if not binary.exists():
        shutil.copy2(a.codefast, binary)
    price = prices(a.model, base / "prices.json")
    arms = a.arms.split(",")
    for arm in arms:
        make_home(base / f"home-{arm}", a.model, binary, mod.ARMS[arm]["plugin"])
    ns = argparse.Namespace(tag=a.tag, model=a.model, ids=a.ids, held=a.held, dev=False, d=0, r=0, k=0, n=0, l=0, w=0)
    tasks = [t for t in mod.choose(ns) if t["family"] in READ_ONLY]
    if a.per_family:
        seen = collections.Counter()
        kept = []
        for t in tasks:
            seen[t["family"]] += 1
            if seen[t["family"]] <= a.per_family:
                kept.append(t)
        tasks = kept
    todo = []
    for rep in range(a.reps):
        for t in tasks:
            for arm in arms:
                done = base / t["id"] / (arm if rep == 0 else f"{arm}.{rep}") / "outcome.json"
                if not done.exists():
                    todo.append((t, arm, rep))
    print(f"{len(tasks)} tasks x {len(arms)} arms x {a.reps}, {len(todo)} sessions to run, {a.model} at ${price['input'] * 1e6:.2f} / ${price['cached'] * 1e6:.2f} cached / ${price['output'] * 1e6:.2f} out a million, {a.jobs} at a time", flush=True)
    # The indexes of the shared trees, written by this batch's binary before any session.
    if any(mod.ARMS[arm]["plugin"] for _, arm, _ in todo):
        if any(t["family"] == "K" for t, _, _ in todo):
            subprocess.run([str(binary), "--root", str(mod.LINUX), "status"], capture_output=True, text=True, env=dict(os.environ, CODEFAST_INDEX_DIR=str(mod.LINUX_INDEX)))
        for tree in sorted({t["repo_dir"] for t, _, _ in todo if t["family"] == "F"}):
            subprocess.run([str(binary), "--root", tree, "status"], capture_output=True, text=True, env=dict(os.environ, CODEFAST_INDEX_DIR=str(mod.f_index(tree))))
        for where, index in {mod.repo_of((t.get("parts") or [t])[0]) for t, _, _ in todo if t["family"] in ("N", "L", "E", "C")}:
            subprocess.run([str(binary), "--root", str(where), "index"], capture_output=True, text=True, env=dict(os.environ, CODEFAST_INDEX_DIR=str(index)))
    rows = [mod.prepare(t, arm, rep, ns, binary) for t, arm, rep in todo]
    with concurrent.futures.ThreadPoolExecutor(a.jobs) as ex:
        futs = [ex.submit(session, row, a, mod, binary, codex) for row in rows]
        for f in concurrent.futures.as_completed(futs):
            try:
                r = f.result()
            except Exception as e:
                print(f"  a session failed: {type(e).__name__}: {e}", flush=True)
                continue
            cost = (r["input"] - r["cached"]) * price["input"] + r["cached"] * price["cached"] + r["output"] * price["output"]
            print(f"  {r['id']:28} {r['arm']:8} exit={r['exit']} passed={r['passed']} score={r.get('kscore')} calls={r['tool_calls']:2d} cf={r['cf_mcp'] + r['cf_cli']:2d} "
                  f"in={r['input']:8,d} cached={r['cached']:8,d} out={r['output']:6,d} ${cost:.3f} {r['seconds']}s" + (f" ERR {r['errors'][0][:120]}" if r["errors"] else ""), flush=True)


def cmd_verify(a):
    """Check the answer of every finished session of a tag again (after a check could not run)."""
    mod = tier2()
    for f in sorted((ROOT / "target/live" / a.tag).glob("*/*/outcome.json")):
        res = json.load(open(f))
        if res.get("kscore") is not None and not a.force:
            continue
        row = json.load(open(f.parent / "task.json"))
        row.setdefault("run", str(f.parent))
        ok, clean = mod.verify(row)
        res.update(passed=ok, clean=clean, **{k: row[k] for k in ("kscore", "rank", "answered") if k in row})
        json.dump(res, open(f, "w"), indent=1)
        print(f"  {res['id']:28} {res['arm']:8} passed={ok} score={res.get('kscore')}", flush=True)


def cmd_score(a):
    base = ROOT / "target/live" / a.tag
    price = json.load(open(base / "prices.json"))
    rows = []
    for f in sorted(base.glob("*/*/outcome.json")):
        r = json.load(open(f))
        r["cost"] = (r["input"] - r["cached"]) * price["input"] + r["cached"] * price["cached"] + r["output"] * price["output"]
        r["tokens"] = r["input"] + r["output"]
        r["score"] = float(r["kscore"]) if r.get("kscore") is not None else (1.0 if r["passed"] else 0.0)
        r["date"] = time.strftime("%Y-%m-%d", time.localtime(f.stat().st_mtime))
        rows.append(r)
    if a.out:
        keep = ("id", "family", "arm", "rep", "repo_name", "model", "exit", "seconds", "passed", "clean", "score", "rank", "input", "cached", "output", "reasoning", "tokens", "cost", "tool_calls", "cf_mcp", "cf_cli", "items", "date")
        with open(a.out, "w") as fh:
            for r in rows:
                fh.write(json.dumps({k: r[k] for k in keep if k in r} | {"harness": "codex", "price": price}) + "\n")
    by = collections.defaultdict(lambda: collections.defaultdict(list))
    for r in rows:
        by[r["family"]][r["id"]].append(r)
    mean = lambda xs: sum(xs) / len(xs) if xs else 0.0
    print(f"{'family':8s} {'tasks':>5s}  {'right, without':>14s} {'with':>6s}  {'tokens, without':>15s} {'with':>10s} {'ratio':>6s}  {'dollars, without':>16s} {'with':>7s} {'ratio':>6s} {'95% of the ratio':>18s}  {'calls':>11s} {'codefast used':>13s}")
    allt = {}
    for fam in sorted(by) + ["all"]:
        tasks = allt if fam == "all" else {t: rs for t, rs in by[fam].items() if {r["arm"] for r in rs} >= {"control", "plugin"}}
        if fam != "all":
            allt.update({fam + ":" + t: rs for t, rs in tasks.items()})
        ids = sorted(tasks)
        if not ids:
            continue
        g = lambda t, arm, k: mean([r[k] for r in tasks[t] if r["arm"] == arm])
        m = lambda arm, k, s=None: mean([g(t, arm, k) for t in (s or ids)])
        random.seed(5)
        boot = sorted(m("plugin", "cost", s) / max(m("control", "cost", s), 1e-12) for s in ([random.choice(ids) for _ in ids] for _ in range(5000)))
        used = mean([1.0 if any(r["cf_mcp"] + r["cf_cli"] for r in tasks[t] if r["arm"] == "plugin") else 0.0 for t in ids])
        print(f"{fam:8s} {len(ids):5d}  {m('control', 'score'):14.3f} {m('plugin', 'score'):6.3f}  {m('control', 'tokens'):15,.0f} {m('plugin', 'tokens'):10,.0f} {m('plugin', 'tokens') / max(m('control', 'tokens'), 1):6.2f}  "
              f"{m('control', 'cost'):16.4f} {m('plugin', 'cost'):7.4f} {m('plugin', 'cost') / max(m('control', 'cost'), 1e-12):6.2f} {boot[125]:8.2f} to {boot[4875]:.2f}  {m('control', 'tool_calls'):5.1f}/{m('plugin', 'tool_calls'):5.1f} {used:13.0%}")


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("run")
    p.add_argument("--tag", required=True)
    p.add_argument("--held", default="", help="held-out sets of bench/tier2/tasks.json, by name: N,C,E,K2,K,F")
    p.add_argument("--ids", default="")
    p.add_argument("--per-family", type=int, default=0, help="at most this many tasks of a family")
    p.add_argument("--arms", default="control,plugin")
    p.add_argument("--model", default="openai/gpt-6.1-sol", help="the model, as the provider names it")
    p.add_argument("--reps", type=int, default=1)
    p.add_argument("--jobs", type=int, default=2)
    p.add_argument("--timeout", type=int, default=1200)
    p.add_argument("--codex", default="codex")
    p.add_argument("--codefast", default=os.environ.get("CODEFAST_BIN") or str(ROOT / "target/release/codefast"))
    p.set_defaults(fn=cmd_run)
    p = sub.add_parser("verify")
    p.add_argument("--tag", required=True)
    p.add_argument("--force", action="store_true", help="also the sessions that have a score")
    p.set_defaults(fn=cmd_verify)
    p = sub.add_parser("score")
    p.add_argument("--tag", required=True)
    p.add_argument("--out", default="")
    p.set_defaults(fn=cmd_score)
    a = ap.parse_args()
    a.fn(a)


if __name__ == "__main__":
    main()
