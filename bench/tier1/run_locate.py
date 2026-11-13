# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Tier 1: localisation without a model. Task text in, ranked files and functions out.

For every task: materialise the source tree at the task's commit, index it, pass the task text to
`codefast locate`, and score the ranking against the files and functions the real fix touched.
Results are appended per task, so an interrupted run resumes where it stopped.
"""
import argparse, hashlib, json, os, shutil, subprocess, sys, time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CF = os.environ.get("CODEFAST_BIN", str(ROOT / "target/release/codefast"))
LOADERS = {
    "linuxflbench": "load_linuxflbench.py",
    "locbench": "load_locbench.py",
    "contextbench": "load_contextbench.py",
    "live-kbench": "load_live_kbench.py",
}


def load(dataset, extra_args):
    cmd = ["uv", "run", str(ROOT / "bench/scripts/datasets" / LOADERS[dataset])] + extra_args
    out = subprocess.run(cmd, capture_output=True, text=True, check=True).stdout
    return [json.loads(l) for l in out.splitlines() if l.strip()]


def is_dev(task_id: str) -> bool:
    """Deterministic half split: tuning looks only at the dev half."""
    return int(hashlib.sha256(task_id.encode()).hexdigest(), 16) % 2 == 0


def prepare_tree(dataset, task_id):
    p = subprocess.run([str(ROOT / "bench/scripts/datasets/prepare_tree.sh"), dataset, task_id], capture_output=True, text=True)
    if p.returncode != 0:
        return None, p.stderr[-400:]
    path = p.stdout.strip().splitlines()[-1] if p.stdout.strip() else ""
    return (path if path and os.path.isdir(path) else None), p.stderr[-400:]


def locate(tree, index_dir, text, n):
    env = dict(os.environ, CODEFAST_INDEX_DIR=index_dir)
    t0 = time.time()
    p = subprocess.run([CF, "--root", tree, "--budget", "0", "--tests", "locate", "-", "--plain", "-n", str(n)], input=text, capture_output=True, text=True, env=env, timeout=600)
    files, syms = [], []
    for line in p.stdout.splitlines():
        parts = line.split("\t")
        if parts[0] == "F" and len(parts) >= 2:
            files.append(parts[1])
        elif parts[0] == "S" and len(parts) >= 3:
            syms.append((parts[1], parts[2]))
    return files, syms, time.time() - t0, p.stderr[-300:]


def score(task, files, syms):
    gold = [g for g in task["gold_files"]]
    ranks = [files.index(g) + 1 if g in files else None for g in gold]
    found = [r for r in ranks if r is not None]
    row = {"n_gold": len(gold), "ranks": ranks}
    for k in (1, 5, 10, 20):
        row[f"any@{k}"] = any(r <= k for r in found)
        row[f"strict@{k}"] = bool(gold) and all(r is not None and r <= k for r in ranks)
        row[f"recall@{k}"] = (sum(1 for r in found if r <= k) / len(gold)) if gold else 0.0
    row["served"] = len(files)
    row["rr"] = (1.0 / min(found)) if found else 0.0
    # function level: gold "file:name" (name may be Class.method)
    gf = task.get("gold_functions") or []
    if gf:
        want = set()
        for g in gf:
            f, _, name = g.rpartition(":")
            want.add((f, name.split(".")[-1]))
        franks = []
        for (f, name) in want:
            r = next((i + 1 for i, (sf, sn) in enumerate(syms) if sf == f and sn == name), None)
            franks.append(r)
        ffound = [r for r in franks if r is not None]
        for k in (1, 5, 10):
            row[f"fn_any@{k}"] = any(r <= k for r in ffound)
        row["fn_rr"] = (1.0 / min(ffound)) if ffound else 0.0
    return row


def run_group(dataset, tasks, args):
    """All tasks that share one source tree."""
    first = tasks[0]
    tree, err = prepare_tree(dataset, first["id"])
    rows = []
    if tree is None:
        for t in tasks:
            rows.append({"id": t["id"], "error": f"tree: {err}"})
        return rows
    key = os.path.basename(tree.rstrip("/"))
    index_dir = str(ROOT / "target/bench-index" / key)
    os.makedirs(index_dir, exist_ok=True)
    for t in tasks:
        try:
            files, syms, secs, err = locate(tree, index_dir, t["query_text"], args.top)
            row = {"id": t["id"], "tree": key, "secs": round(secs, 3), "dev": is_dev(t["id"]), "top": files[:10]}
            row.update(score(t, files, syms))
            if not files:
                row["error"] = err
        except Exception as e:  # noqa: BLE001
            row = {"id": t["id"], "error": repr(e)[:300]}
        rows.append(row)
    if not args.keep:
        shutil.rmtree(index_dir, ignore_errors=True)
        if "/_trees/" in tree and args.drop_trees:
            shutil.rmtree(tree, ignore_errors=True)
    return rows


def summarise(rows, label):
    ok = [r for r in rows if "error" not in r]
    if not ok:
        print(f"{label}: no results")
        return
    n = len(ok)
    m = lambda k: sum(1 for r in ok if r.get(k)) / n
    mrr = sum(r["rr"] for r in ok) / n
    line = f"{label:18} n={n:4}  any@1 {m('any@1'):.3f}  any@5 {m('any@5'):.3f}  any@10 {m('any@10'):.3f}  strict@10 {m('strict@10'):.3f}  MRR {mrr:.3f}"
    fn = [r for r in ok if "fn_rr" in r]
    if fn:
        line += f"  | fn any@1 {sum(1 for r in fn if r['fn_any@1'])/len(fn):.3f} any@5 {sum(1 for r in fn if r['fn_any@5'])/len(fn):.3f} any@10 {sum(1 for r in fn if r['fn_any@10'])/len(fn):.3f} MRR {sum(r['fn_rr'] for r in fn)/len(fn):.3f}"
    cov = lambda k: sum(r.get(f"recall@{k}", 0.0) for r in ok) / n
    line += f"  | coverage@5 {cov(5):.3f} @10 {cov(10):.3f} @20 {cov(20):.3f}"
    line += f"  | {sum(r['secs'] for r in ok)/n:.2f}s/query, errors {len(rows)-n}"
    print(line)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dataset", required=True, choices=list(LOADERS))
    ap.add_argument("--loader-args", default="", help="extra arguments for the dataset loader, e.g. '--subset ripwire-r4-60'")
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--only", choices=["dev", "test", "all"], default="all")
    ap.add_argument("--jobs", type=int, default=2)
    ap.add_argument("--top", type=int, default=20)
    ap.add_argument("--out", default="")
    ap.add_argument("--keep", action="store_true", help="keep indexes")
    ap.add_argument("--drop-trees", action="store_true", help="delete each source tree after use")
    ap.add_argument("--fresh", action="store_true", help="ignore previous results")
    a = ap.parse_args()
    tasks = load(a.dataset, a.loader_args.split())
    if a.only != "all":
        tasks = [t for t in tasks if is_dev(t["id"]) == (a.only == "dev")]
    if a.limit:
        tasks = tasks[: a.limit]
    out = a.out or str(ROOT / f"bench/results/tier1-{a.dataset}.jsonl")
    os.makedirs(os.path.dirname(out), exist_ok=True)
    done = {}
    if os.path.exists(out) and not a.fresh:
        for l in open(out):
            r = json.loads(l)
            if "error" not in r:
                done[r["id"]] = r
    todo = [t for t in tasks if t["id"] not in done]
    groups = {}
    for t in todo:
        groups.setdefault((t["repo"], t["commit_or_version"]), []).append(t)
    print(f"{len(tasks)} tasks, {len(done)} cached, {len(todo)} to run in {len(groups)} trees", flush=True)
    rows = list(done.values())
    with open(out, "w" if a.fresh else "a") as fo, ThreadPoolExecutor(max_workers=a.jobs) as ex:
        for i, res in enumerate(ex.map(lambda g: run_group(a.dataset, g, a), groups.values())):
            for r in res:
                fo.write(json.dumps(r) + "\n")
                rows.append(r)
            fo.flush()
            if (i + 1) % 10 == 0:
                print(f"  {i + 1}/{len(groups)} trees", flush=True)
    ids = {t["id"] for t in tasks}
    rows = [r for r in rows if r["id"] in ids]
    summarise(rows, "all")
    summarise([r for r in rows if r.get("dev")], "dev half")
    summarise([r for r in rows if r.get("dev") is False], "held-out half")


if __name__ == "__main__":
    main()
