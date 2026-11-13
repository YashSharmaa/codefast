"""Offline check of `codefast rename` on the Go rename tasks of the live benchmark: the rename
is applied to a fresh copy of syft and judged by the task's own check (`go build` and `go vet`).
No model is involved.

usage: python3 bench/tier2/rename_offline.py [--tasks r] [--out FILE]
Needs bench/repos/syft, target/ab/r/tasks.json (make_r_tasks.py) and Go. CODEFAST_BIN names the
binary (default target/release/codefast). `--tasks r2` takes the tasks of another repository
(`make_r_tasks.py --out r2`), which name their checkout themselves.
"""
import argparse, json, os, subprocess, tempfile, time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SYFT = ROOT / "bench/repos/syft"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--tasks", default="r", help="directory under target/ab")
    ap.add_argument("--out", default=str(ROOT / "bench/results/rename-offline.jsonl"))
    a = ap.parse_args()
    binary = os.environ.get("CODEFAST_BIN", str(ROOT / "target/release/codefast"))
    tasks = json.load(open(ROOT / "target/ab" / a.tasks / "tasks.json"))
    work = Path(tempfile.mkdtemp(prefix="codefast-rename-", dir=ROOT / "target"))
    rows = []
    for t in tasks:
        repo = work / t["id"]
        src = t.get("repo_dir") or SYFT
        subprocess.run(["rsync", "-a", "--exclude", ".git", "--exclude", ".codefast", f"{src}/", f"{repo}/"], check=True)
        owner = t["receiver"].lstrip("*")
        env = dict(os.environ, CODEFAST_INDEX_DIR=str(work / f"index-{t['id']}"))
        t0 = time.time()
        p = subprocess.run([binary, "--root", str(repo), "rename", f"{owner}.{t['name']}", t["new_name"]], capture_output=True, text=True, env=env)
        secs = time.time() - t0
        said = p.stdout.splitlines()
        check = subprocess.run(t["check_cmd"].format(repo=repo), shell=True, capture_output=True, text=True)
        left = [l.strip() for l in said if l.startswith("  ")]
        row = dict(id=t["id"], target=f"{owner}.{t['name']}", new_name=t["new_name"], passed=check.returncode == 0 and bool(said) and said[0].startswith("renamed "), rename_seconds=round(secs, 2),
                   said=said[0] if said else "", listed_as_not_changed=left, compiler=(check.stdout + check.stderr).strip()[-600:] if check.returncode else "")
        rows.append(row)
        print(f"{t['id']:26} passed={row['passed']}  {row['said']}", flush=True)
    with open(a.out, "w") as fo:
        for r in rows:
            fo.write(json.dumps(r) + "\n")
    print(f"{sum(r['passed'] for r in rows)} of {len(rows)} pass the compiler after the one command. Work directory: {work}")


main()
