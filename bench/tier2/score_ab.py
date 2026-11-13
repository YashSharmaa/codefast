# /// script
# requires-python = ">=3.12"
# dependencies = ["tiktoken"]
# ///
"""Tier 2 pilot: score paired agent runs (control vs tool) from their transcripts.

Each run is one sub-agent transcript (`agent-<id>.jsonl` with `agent-<id>.meta.json`) whose label is
`<tag>:<task id>:<arm>`. From the transcript we take, per run:

  * the final structured answer (the last StructuredOutput call),
  * token usage summed over distinct API responses (uncached input, cache read, cache write, output),
  * tool calls by name, how many shell commands invoked the codefast wrapper (adoption),
  * rule violations (web, sub-agents, the control arm touching the tool).

Kernel questions are scored with the K3 scorer on the answer text. Localisation tasks are scored
against the gold files (all gold in the top 10, any gold in the top 10).

usage: uv run bench/tier2/score_ab.py TRANSCRIPT_DIR [TRANSCRIPT_DIR ...] [--out results.jsonl]
"""
import argparse, glob, hashlib, importlib.util, json, os, re, statistics, subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
AB = ROOT / "target/ab"

# Price of each token class relative to one uncached input token (Anthropic list prices keep these
# ratios across tiers: cache read 0.1x, 5-minute cache write 1.25x, output 5x).
W_READ, W_WRITE, W_OUT = 0.1, 1.25, 5.0


def load_k3():
    spec = importlib.util.spec_from_file_location("run_k3", ROOT / "bench/k3/run_k3.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def parse_run(path):
    meta = json.load(open(path.replace(".jsonl", ".meta.json")))
    label = meta.get("description", "")
    usage = dict(input=0, cache_read=0, cache_write=0, output=0)
    seen, tools, cmds = set(), {}, []
    answer, first_ts, last_ts, requests = None, None, None, 0
    model = None
    result_chars = 0
    with open(path) as fh:
        for line in fh:
            try:
                o = json.loads(line)
            except Exception:
                continue
            ts = o.get("timestamp")
            if ts:
                first_ts = first_ts or ts
                last_ts = ts
            if o.get("type") == "user":
                # what tool calls put into the context
                for b in ((o.get("message") or {}).get("content") or []) if isinstance((o.get("message") or {}).get("content"), list) else []:
                    if isinstance(b, dict) and b.get("type") == "tool_result":
                        c = b.get("content")
                        if isinstance(c, str):
                            result_chars += len(c)
                        elif isinstance(c, list):
                            result_chars += sum(len(x.get("text", "")) for x in c if isinstance(x, dict))
            if o.get("type") != "assistant":
                continue
            m = o.get("message") or {}
            mid = m.get("id")
            if mid and mid not in seen and "usage" in m:
                seen.add(mid)
                u = m["usage"]
                usage["input"] += u.get("input_tokens") or 0
                usage["cache_read"] += u.get("cache_read_input_tokens") or 0
                usage["cache_write"] += u.get("cache_creation_input_tokens") or 0
                usage["output"] += u.get("output_tokens") or 0
                requests += 1
                model = m.get("model") or model
            for b in m.get("content") or []:
                if isinstance(b, dict) and b.get("type") == "tool_use":
                    name = b.get("name", "?")
                    tools[name] = tools.get(name, 0) + 1
                    inp = b.get("input") or {}
                    if name == "Bash":
                        cmds.append(inp.get("command", ""))
                    if name == "StructuredOutput":
                        answer = inp
    return dict(label=label, usage=usage, tools=tools, cmds=cmds, answer=answer, requests=requests, model=model, first_ts=first_ts, last_ts=last_ts, result_chars=result_chars)


def words(s):
    return re.findall(r"[A-Za-z_][A-Za-z0-9_]*", s)


def score_k(k3, qn, text):
    """K3 scoring, except that a signature fragment counts when its identifiers all occur in one
    sentence of the answer: agents describe fields in prose instead of quoting the declaration."""
    s, detail = k3.score(qn, text)
    if qn["family"] == "F2" and s < 1.0:
        chunks = re.split(r"[\n.;]", text)
        hits = 0
        for item in qn["gold"]["must_contain"]:
            ws = words(item)
            if k3.collapse(item) in k3.collapse(text) or any(all(re.search(r"(?<![A-Za-z0-9_])" + re.escape(w) + r"(?![A-Za-z0-9_])", c) for w in ws) for c in chunks):
                hits += 1
        s = hits / len(qn["gold"]["must_contain"])
    return s


def test_hash(repo):
    h = hashlib.sha256()
    for p in sorted(Path(repo).rglob("*")):
        rel = str(p.relative_to(repo))
        if p.is_file() and (rel.endswith("_test.go") or rel.startswith("testdata/") or "/testdata/" in rel):
            h.update(rel.encode())
            h.update(p.read_bytes())
    return h.hexdigest()


def verify_w(t):
    """Run the package tests in the task's copy. Returns (passed, tests untouched)."""
    try:
        # TestDetectWithSymlinks depends on a symlink that the task copies hold as a regular file,
        # so it fails in every copy whatever the agent wrote.
        p = subprocess.run(["go", "test", f"./{t['pkg']}/...", "-skip", "^TestDetectWithSymlinks$"], cwd=t["repo"], capture_output=True, text=True, timeout=600)
        ok = p.returncode == 0
    except Exception:
        ok = False
    return ok, test_hash(t["repo"]) == t["tests"]


def seconds(a, b):
    from datetime import datetime
    try:
        f = lambda s: datetime.fromisoformat(s.replace("Z", "+00:00"))
        return (f(b) - f(a)).total_seconds()
    except Exception:
        return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dirs", nargs="+")
    ap.add_argument("--out", default="")
    a = ap.parse_args()
    k3 = load_k3()
    questions = {json.loads(l)["id"]: json.loads(l) for l in open(ROOT / "bench/k3/questions.verified.jsonl")}
    tasks = {t["id"]: t for t in json.load(open(AB / "tasks.json"))}
    wtasks = {(t["id"], t["arm"]): t for t in json.load(open(AB / "w/tasks.json"))} if (AB / "w/tasks.json").exists() else {}
    tool_marker = str(AB / "tools")
    rows = []
    for d in a.dirs:
        for path in sorted(glob.glob(os.path.join(d, "agent-*.jsonl"))):
            r = parse_run(path)
            parts = r["label"].split(":")
            if len(parts) != 3:
                continue
            tag, tid, arm = parts
            t = tasks.get(tid) or wtasks.get((tid, arm))
            if t is None:
                continue
            ans = r["answer"] or {}
            text = (ans.get("answer") or "") if isinstance(ans, dict) else ""
            files = ans.get("files") or [] if isinstance(ans, dict) else []
            u = r["usage"]
            row = dict(tag=tag, id=tid, kind=t["kind"], arm=arm, model=r["model"], requests=r["requests"], **u)
            row["processed"] = u["input"] + u["cache_read"] + u["cache_write"]
            row["weighted"] = round(u["input"] + W_READ * u["cache_read"] + W_WRITE * u["cache_write"] + W_OUT * u["output"])
            row["tool_calls"] = sum(v for k, v in r["tools"].items() if k != "StructuredOutput")
            row["cf_calls"] = sum(1 for c in r["cmds"] if tool_marker in c)
            row["result_tokens"] = r["result_chars"] // 4
            row["secs"] = seconds(r["first_ts"], r["last_ts"])
            row["answered"] = r["answer"] is not None
            banned = ("WebSearch", "WebFetch", "Agent", "Workflow") + (() if t["kind"] == "W" else ("Write", "Edit"))
            other = [k for k in r["tools"] if k in banned]
            outside = [c for c in r["cmds"] if "codefast" in c.replace(t["repo"], "").replace(tool_marker, "") and arm == "control"]
            row["violations"] = other + (["control arm referenced the tool"] if (arm == "control" and (row["cf_calls"] or outside)) else [])
            if t["kind"] == "K":
                row["score"] = round(score_k(k3, questions[tid], text), 3) if text else 0.0
            elif t["kind"] == "W":
                passed, untouched = verify_w(t)
                row["tests_pass"], row["tests_untouched"] = passed, untouched
                row["score"] = 1.0 if (passed and untouched) else 0.0
                stray = [c for c in r["cmds"] if "bench/repos/gitleaks" in c or re.search(r"/w/W\d+/(?!%s/)" % re.escape(arm), c)]
                if stray:
                    row["violations"] = row["violations"] + ["looked outside its copy"]
            else:
                gold = t["gold_files"]
                norm = [f.strip().replace(t["repo"].rstrip("/") + "/", "").lstrip("./") for f in files[:10]]
                ranks = [norm.index(g) + 1 for g in gold if g in norm]
                row["rank"] = min(ranks) if ranks else None
                row["any@1"] = bool(ranks) and min(ranks) == 1
                row["strict@10"] = bool(gold) and all(g in norm for g in gold)
                row["any@10"] = any(g in norm for g in gold)
                row["score"] = 1.0 if row["strict@10"] else 0.0
            rows.append(row)
    if a.out:
        with open(a.out, "w") as fo:
            for r in rows:
                fo.write(json.dumps(r) + "\n")
    report(rows)


def report(rows):
    by = {}
    for r in rows:
        by.setdefault((r["id"], r["arm"]), r)
    ids = sorted({r["id"] for r in rows})
    print(f"{len(rows)} runs, models: {sorted({r['model'] for r in rows if r['model']})}")
    viol = [r for r in rows if r["violations"]]
    for r in viol:
        print(f"  violation {r['id']} {r['arm']}: {r['violations']}")
    for arm in sorted({r["arm"] for r in rows} - {"control"}):
        pairs = [(by[(i, "control")], by[(i, arm)]) for i in ids if (i, "control") in by and (i, arm) in by]
        print(f"\n######## arm '{arm}' against control: {len(pairs)} complete pairs")
        compare(pairs, arm)


def compare(pairs, arm_name):
    for kind in ("K", "L", "W", "all"):
        ps = [p for p in pairs if kind == "all" or p[0]["kind"] == kind]
        if not ps:
            continue
        print(f"\n== {kind}: {len(ps)} pairs ==")
        hdr = f"{'arm':8} {'score':>6} {'processed':>10} {'output':>7} {'weighted':>9} {'requests':>8} {'tools':>6} {'cf':>4} {'secs':>6} {'results':>8}"
        print(hdr)
        for j, arm in enumerate(("control", arm_name)):
            rs = [p[j] for p in ps]
            mean = lambda k: statistics.mean((r[k] or 0) for r in rs)
            print(f"{arm:8} {mean('score'):6.3f} {mean('processed'):10.0f} {mean('output'):7.0f} {mean('weighted'):9.0f} {mean('requests'):8.1f} {mean('tool_calls'):6.1f} {mean('cf_calls'):4.1f} {mean('secs'):6.0f} {mean('result_tokens'):8.0f}")
        med = lambda k, j: statistics.median((p[j][k] or 0) for p in ps)
        print(f"median processed tokens: control {med('processed', 0):.0f}, tool {med('processed', 1):.0f}; median weighted: control {med('weighted', 0):.0f}, tool {med('weighted', 1):.0f}")
        tot = lambda k, j: sum((p[j][k] or 0) for p in ps)
        for k in ("processed", "output", "weighted", "tool_calls", "secs", "result_tokens"):
            c, t = tot(k, 0), tot(k, 1)
            if c:
                print(f"  total {k:10}: control {c:12.0f}  tool {t:12.0f}  change {100.0 * (t - c) / c:+.1f}%")
        fewer = sum(1 for c, t in ps if t["weighted"] < c["weighted"])
        print(f"  tool arm cheaper (weighted) in {fewer} of {len(ps)} pairs; tool used in {sum(1 for _, t in ps if t['cf_calls'])} of {len(ps)} tool runs")
        better = sum(1 for c, t in ps if t["score"] > c["score"])
        worse = sum(1 for c, t in ps if t["score"] < c["score"])
        print(f"  score: tool better in {better}, worse in {worse}, equal in {len(ps) - better - worse}")


if __name__ == "__main__":
    main()
