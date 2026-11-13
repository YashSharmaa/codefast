# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Agent experiment, round 3: tasks that end in a change to the code, checked by tests or by the
compiler, run with and without codefast.

Families:
  D  make a failing test pass in a real repository (bench/tier2/make_d_tasks.py)
  R  carry a method rename through every call site (bench/tier2/make_r_tasks.py)

    round3.py arms  --tag r3h --arms control,codefast [--dev] [--d N] [--r N]
        makes a private copy of each task's tree per arm, a wrapper for the codefast command of
        the codefast arm, and target/ab/r3/<tag>.json: what the workflow script needs to run it.

    round3.py score TRANSCRIPT_DIR [...] --tag r3h [--out results.jsonl]
        reads each run's transcript (tokens, requests, tool calls), then checks the outcome in the
        run's own copy: the tests of the task pass and are untouched (D), the project builds and
        only call sites changed (R).

Development tasks (--dev) are a fixed few that the tool and its guide may be tuned on. They are
never part of a reported result.
"""
import argparse, glob, hashlib, importlib.util, json, os, re, shutil, subprocess, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
AB = ROOT / "target/ab"
OUT = AB / "r3"
BIN = AB / "codefast-bin-r3"
SYFT = ROOT / "bench/repos/syft"
DEV = {"D": 2, "R": 1}
W_READ, W_WRITE, W_OUT = 0.1, 1.25, 5.0


def load(name):
    p = AB / name / "tasks.json"
    return json.load(open(p)) if p.exists() else []


def file_hash(repo, files):
    h = hashlib.sha256()
    for f in sorted(files):
        p = Path(repo) / f
        h.update(f.encode())
        h.update(p.read_bytes() if p.exists() else b"<missing>")
    return h.hexdigest()


def split(tasks, fam, dev, n):
    """A fixed order by hash of the id, so that the choice does not depend on anything else."""
    ordered = sorted(tasks, key=lambda t: hashlib.sha256(t["id"].encode()).hexdigest())
    if fam == "D":
        # Alternate the repositories so that one of them does not fill the list.
        by = {}
        for t in ordered:
            by.setdefault(t["repo_name"], []).append(t)
        ordered, groups = [], list(by.values())
        while any(groups):
            for g in groups:
                if g:
                    ordered.append(g.pop(0))
    k = DEV[fam]
    return ordered[:k] if dev else ordered[k:k + n]


def arms(a):
    arms_ = a.arms.split(",")
    OUT.mkdir(parents=True, exist_ok=True)
    if not BIN.exists() or a.refresh_bin:
        shutil.copy2(ROOT / "target/final/codefast", BIN)
    chosen = split(load("d"), "D", a.dev, a.d) + split(load("r"), "R", a.dev, a.r)
    rows = []
    for t in chosen:
        for arm in arms_:
            dst = OUT / a.tag / t["id"] / arm / "repo"
            if dst.exists():
                shutil.rmtree(dst)
            # A rename starts from the project as it is: the agent changes the definition too.
            src = SYFT if t["family"] == "R" else t["base"]
            shutil.copytree(src, dst, symlinks=True, ignore=shutil.ignore_patterns(".git", ".codefast"))
            tool = ""
            if arm != "control":
                tdir = OUT / a.tag / "tools" / f"{t['id']}-{arm}"
                tdir.mkdir(parents=True, exist_ok=True)
                tool = str(tdir / "codefast")
                idx = OUT / a.tag / t["id"] / arm / "index"
                Path(tool).write_text(f'#!/bin/sh\nCODEFAST_INDEX_DIR={idx} exec {BIN} --root {dst} "$@"\n')
                os.chmod(tool, 0o755)
            row = dict(id=t["id"], family=t["family"], arm=arm, repo=str(dst), tool=tool, repo_name=t["repo_name"])
            if t["family"] == "D":
                row.update(issue=t["issue"][:6000], f2p=t["f2p"][:8], test_cmd=t["test_cmd"].replace("{repo}", str(dst)), test_files=t["test_files"],
                           tests_hash=file_hash(dst, t["test_files"]))
            else:
                row.update(receiver=t["receiver"], name=t["name"], new_name=t["new_name"], file=t["file"], line=t["line"], sites=t["sites"],
                           check_cmd=t["check_cmd"].replace("{repo}", str(dst)))
            rows.append(row)
    guide = (ROOT / "skills/codefast/SKILL.md").read_text().split("---", 2)[2].strip()
    # The rules block exactly as `codefast setup agents-md` writes it into a project.
    scratch = OUT / "rules-project"
    if scratch.exists():
        shutil.rmtree(scratch)
    scratch.mkdir(parents=True)
    subprocess.run(["git", "init", "-q", "."], cwd=scratch, check=True)
    subprocess.run([str(BIN), "setup", "agents-md", "--apply"], capture_output=True, text=True, cwd=scratch)
    m = re.search(r"<!-- codefast:begin -->\n(.*?)<!-- codefast:end -->", (scratch / "AGENTS.md").read_text(), re.S)
    cfg = dict(tag=a.tag, tasks=rows, guide=guide, rule=(m.group(1).strip() if m else ""))
    assert cfg["rule"], "could not read the rules block that `codefast setup agents-md` writes"
    json.dump(cfg, open(OUT / f"{a.tag}.json", "w"), indent=1)
    print(len(chosen), "tasks,", len(rows), "runs ->", OUT / f"{a.tag}.json")
    for t in chosen:
        print(" ", t["id"], t["family"], t["repo_name"])


def parse_run(path):
    spec = importlib.util.spec_from_file_location("score_ab", ROOT / "bench/tier2/score_ab.py")
    mod = importlib.util.module_from_spec(spec)
    sys.argv = [sys.argv[0]]
    spec.loader.exec_module(mod)
    return mod.parse_run(path)


def verify_d(t):
    try:
        p = subprocess.run(t["test_cmd"], shell=True, capture_output=True, text=True, timeout=1800)
        ok = p.returncode == 0
    except Exception:
        ok = False
    return ok, file_hash(t["repo"], t["test_files"]) == t["tests_hash"]


def verify_r(t, base):
    try:
        p = subprocess.run(t["check_cmd"], shell=True, capture_output=True, text=True, timeout=1800)
        ok = p.returncode == 0
    except Exception:
        ok = False
    # The method of this one type is renamed, and no other definition is: `base` is the tree
    # the task builder left, with exactly that one definition renamed.
    defs = lambda root: sorted(
        (str(p.relative_to(root)), m.group(0))
        for p in Path(root).rglob("*.go")
        for m in re.finditer(r"^func \([^)]*\) \w+", p.read_text(errors="replace"), re.M)
        if m.group(0).endswith((" " + t["name"], " " + t["new_name"]))
    )
    return ok, defs(t["repo"]) == defs(base)


def score(a):
    cfg = json.load(open(OUT / f"{a.tag}.json"))
    tasks = {(t["id"], t["arm"]): t for t in cfg["tasks"]}
    bases = {t["id"]: t["base"] for t in load("d") + load("r")}
    marker = str(OUT / a.tag / "tools")
    rows = []
    for d in a.dirs:
        for path in sorted(glob.glob(os.path.join(d, "agent-*.jsonl"))):
            r = parse_run(path)
            parts = r["label"].split(":")
            if len(parts) != 3 or parts[0] != a.tag:
                continue
            _, tid, arm = parts
            t = tasks.get((tid, arm))
            if t is None:
                continue
            u = r["usage"]
            row = dict(tag=a.tag, id=tid, family=t["family"], arm=arm, model=r["model"], requests=r["requests"], **u)
            row["processed"] = u["input"] + u["cache_read"] + u["cache_write"]
            row["weighted"] = round(u["input"] + W_READ * u["cache_read"] + W_WRITE * u["cache_write"] + W_OUT * u["output"])
            row["tool_calls"] = sum(v for k, v in r["tools"].items() if k != "StructuredOutput")
            row["cf_calls"] = sum(c.count(marker) for c in r["cmds"])
            row["result_tokens"] = r["result_chars"] // 4
            row["answered"] = r["answer"] is not None
            banned = [k for k in r["tools"] if k in ("WebSearch", "WebFetch", "Agent", "Workflow")]
            row["violations"] = banned + (["control arm used the tool"] if arm == "control" and any("codefast" in c.replace(t["repo"], "") for c in r["cmds"]) else [])
            other_copies = [c for c in r["cmds"] if re.search(r"/r3/[^/]+/[^/]+/(?!%s/)\w+/repo" % re.escape(arm), c) or "/base" in c and "target/ab" in c]
            if other_copies:
                row["violations"].append("looked outside its copy")
            if t["family"] == "D":
                ok, untouched = verify_d(t)
            else:
                ok, untouched = verify_r(t, bases[tid])
            row["passed"], row["clean"] = ok, untouched
            row["score"] = 1.0 if ok and untouched else 0.0
            rows.append(row)
            print(f"{tid:28} {arm:9} score={row['score']:.0f} req={row['requests']:3d} processed={row['processed']/1000:6.0f}k weighted={row['weighted']/1000:6.1f}k cf={row['cf_calls']} {row['violations'] or ''}", flush=True)
    if a.out:
        with open(a.out, "w") as fo:
            for r in rows:
                fo.write(json.dumps(r) + "\n")
    report(rows)


def report(rows):
    fams = sorted(set(r["family"] for r in rows))
    arms_ = sorted(set(r["arm"] for r in rows))
    print()
    print(f"{'family':7} {'arm':9} {'n':>3} {'solved':>6} {'requests':>8} {'processed':>10} {'weighted':>9} {'out tok':>8} {'tools':>6} {'cf':>5} {'adopt':>6}")
    for fam in fams + ["all"]:
        for arm in arms_:
            v = [r for r in rows if (fam == "all" or r["family"] == fam) and r["arm"] == arm]
            if not v:
                continue
            n = len(v)
            m = lambda k: sum(x[k] for x in v) / n
            print(f"{fam:7} {arm:9} {n:3d} {m('score'):6.2f} {m('requests'):8.1f} {m('processed')/1000:9.0f}k {m('weighted')/1000:8.1f}k {m('output'):8.0f} {m('tool_calls'):6.1f} {m('cf_calls'):5.1f} {sum(1 for x in v if x['cf_calls'])/n:6.2f}")
    # Paired: the same task in both arms, and separately the tasks both arms solved.
    if len(arms_) == 2:
        a0, a1 = ("control", [x for x in arms_ if x != "control"][0]) if "control" in arms_ else arms_
        by = {}
        for r in rows:
            by.setdefault(r["id"], {})[r["arm"]] = r
        for name, keep in (("all pairs", lambda p: True), ("solved by both", lambda p: p[a0]["score"] == 1.0 and p[a1]["score"] == 1.0)):
            for fam in fams + ["all"]:
                pairs = [p for p in by.values() if a0 in p and a1 in p and (fam == "all" or p[a0]["family"] == fam) and keep(p)]
                if not pairs:
                    continue
                tot = lambda arm, k: sum(p[arm][k] for p in pairs)
                wins = sum(1 for p in pairs if p[a1]["weighted"] < p[a0]["weighted"])
                print(f"{name:15} {fam:4} n={len(pairs):2d}  weighted {a1}/{a0} = {tot(a1,'weighted')/max(tot(a0,'weighted'),1):.2f}  processed = {tot(a1,'processed')/max(tot(a0,'processed'),1):.2f}"
                      f"  requests = {tot(a1,'requests')/max(tot(a0,'requests'),1):.2f}  output = {tot(a1,'output')/max(tot(a0,'output'),1):.2f}  cheaper in {wins}/{len(pairs)}")


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("arms")
    p.add_argument("--tag", required=True)
    p.add_argument("--arms", default="control,codefast")
    p.add_argument("--dev", action="store_true")
    p.add_argument("--d", type=int, default=16)
    p.add_argument("--r", type=int, default=8)
    p.add_argument("--refresh-bin", action="store_true")
    p = sub.add_parser("score")
    p.add_argument("dirs", nargs="+")
    p.add_argument("--tag", required=True)
    p.add_argument("--out", default="")
    a = ap.parse_args()
    arms(a) if a.cmd == "arms" else score(a)


if __name__ == "__main__":
    main()
