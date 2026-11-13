# /// script
# requires-python = ">=3.12"
# dependencies = ["pyarrow", "pandas"]
# ///
"""Agent experiment, family D: make a failing test pass in a real repository.

Each task is one Loc-Bench instance (a real issue with the fix that was merged and the tests that
came with it). The repository is checked out at the commit before the fix, the tests of the fix
are added, and the task is valid when at least one of those tests fails there and passes once the
real fix is applied. The agent gets the issue text and the names of the failing tests, and has to
change the source until they pass. Nothing of the real fix is shown.

For every valid task this writes `target/ab/d/<id>/base` (the tree the agent starts from) and a
row in `target/ab/d/tasks.json` with the command that runs the tests.

usage: uv run bench/tier2/make_d_tasks.py [--repos django/django,tobymao/sqlglot] [--limit N]
"""
import argparse, json, os, re, shutil, subprocess, sys
import xml.etree.ElementTree as ET
from pathlib import Path

import pandas as pd

ROOT = Path(__file__).resolve().parents[2]
AB = ROOT / "target/ab/d"
PARQUET = ROOT / "bench/data/locbench/hf-czlll-Loc-Bench_V1/data/test-00000-of-00001.parquet"
PREPARE = ROOT / "bench/scripts/datasets/prepare_tree.sh"

ENVS = {
    "django/django": ("env-django", ["asgiref", "sqlparse", "tzdata", "jinja2", "pyyaml", "tblib", "docutils", "pillow", "numpy", "bcrypt", "argon2-cffi"]),
    "tobymao/sqlglot": ("env-sqlglot", ["pytest", "python-dateutil", "duckdb", "pandas", "numpy"]),
}


def sh(cmd, cwd=None, inp=None, env=None, timeout=900):
    try:
        p = subprocess.run(cmd, cwd=cwd, input=inp, capture_output=True, text=True, env=env, timeout=timeout)
    except subprocess.TimeoutExpired:
        # A test that hangs before the fix (a performance report) is not a usable task.
        return 124, "", "timed out"
    return p.returncode, p.stdout, p.stderr


def diff_files(patch):
    return re.findall(r"^diff --git a/(\S+) b/", patch or "", re.M)


def ensure_env(repo):
    name, pkgs = ENVS[repo]
    env = AB / name
    if not (env / "bin/python").exists():
        AB.mkdir(parents=True, exist_ok=True)
        rc, _, err = sh(["uv", "venv", "--python", "3.12", str(env)])
        assert rc == 0, err
        rc, _, err = sh(["uv", "pip", "install", "--python", str(env / "bin/python"), *pkgs])
        assert rc == 0, err
    return env


def copy_tree(src, dst):
    if dst.exists():
        shutil.rmtree(dst)
    shutil.copytree(src, dst, symlinks=True, ignore=shutil.ignore_patterns(".git", ".codefast", "__pycache__"))


def apply(patch, cwd):
    rc, out, err = sh(["patch", "-p1", "-s", "--no-backup-if-mismatch", "-f"], cwd=cwd, inp=patch)
    return rc == 0, (out + err)[-400:]


def run_django(repo_dir, env, labels):
    e = dict(os.environ, PYTHONPATH=str(repo_dir), PYTHONDONTWRITEBYTECODE="1")
    rc, out, err = sh([str(env / "bin/python"), "runtests.py", "--parallel", "1", "-v", "1", *labels], cwd=repo_dir / "tests", env=e, timeout=600)
    text = out + "\n" + err
    failed = sorted(set(re.findall(r"^(?:FAIL|ERROR): \S+ \(([\w.]+)\)", text, re.M)))
    m = re.search(r"^Ran (\d+) tests? in", text, re.M)
    ran = int(m.group(1)) if m else 0
    ok = bool(re.search(r"^OK", text, re.M)) and rc == 0
    return dict(ran=ran, ok=ok, failed=failed, tail=text[-600:])


def run_pytest(repo_dir, env, files):
    xml = repo_dir / ".d-junit.xml"
    e = dict(os.environ, PYTHONPATH=str(repo_dir), PYTHONDONTWRITEBYTECODE="1")
    rc, out, err = sh([str(env / "bin/python"), "-m", "pytest", "-q", "-x", "--no-header", "-p", "no:cacheprovider", f"--junitxml={xml}", *files], cwd=repo_dir, env=e, timeout=300)
    failed, ran = [], 0
    if xml.exists():
        for tc in ET.parse(xml).getroot().iter("testcase"):
            ran += 1
            if tc.find("failure") is not None or tc.find("error") is not None:
                failed.append(f"{tc.get('classname')}.{tc.get('name')}")
        xml.unlink()
    return dict(ran=ran, ok=rc == 0, failed=sorted(failed), tail=(out + err)[-600:])


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--repos", default="django/django,tobymao/sqlglot")
    ap.add_argument("--limit", type=int, default=0)
    a = ap.parse_args()
    repos = a.repos.split(",")
    df = pd.read_parquet(PARQUET)
    tasks, log = [], []
    for _, r in df.iterrows():
        repo = r["repo"]
        if repo not in repos or not r["test_patch"]:
            continue
        iid = r["instance_id"]
        tfiles = [f for f in diff_files(r["test_patch"]) if f.endswith(".py")]
        sfiles = [f for f in diff_files(r["patch"]) if f.endswith(".py") and not re.search(r"(^|/)tests?/", f)]
        if not tfiles or not sfiles or len(sfiles) > 3 or int(pd.to_datetime(r["created_at"], unit="ms").year) < 2024:
            continue
        rc, out, err = sh(["bash", str(PREPARE), "--repo", repo, "--commit", r["base_commit"]], timeout=1800)
        if rc != 0:
            log.append((iid, "no tree", err[-200:]))
            continue
        tree = Path(out.strip().splitlines()[-1])
        env = ensure_env(repo)
        base, gold = AB / iid / "base", AB / iid / "gold"
        copy_tree(tree, base)
        ok, msg = apply(r["test_patch"], base)
        if not ok:
            log.append((iid, "test patch does not apply", msg))
            continue
        copy_tree(base, gold)
        ok, msg = apply(r["patch"], gold)
        if not ok:
            log.append((iid, "fix does not apply", msg))
            continue
        if repo == "django/django":
            labels = []
            for f in tfiles:
                if f.startswith("tests/"):
                    labels.append(f[len("tests/"):-3].replace("/", "."))
            if not labels:
                continue
            before, after = run_django(base, env, labels), run_django(gold, env, labels)
            cmd = f"cd {{repo}}/tests && PYTHONPATH={{repo}} {env}/bin/python runtests.py --parallel 1 " + " ".join(labels)
        else:
            before, after = run_pytest(base, env, tfiles), run_pytest(gold, env, tfiles)
            cmd = f"cd {{repo}} && PYTHONPATH={{repo}} {env}/bin/python -m pytest -q -p no:cacheprovider " + " ".join(tfiles)
        shutil.rmtree(gold, ignore_errors=True)
        f2p = [t for t in before["failed"] if t not in after["failed"]]
        valid = bool(f2p) and after["ran"] > 0 and not after["failed"]
        log.append((iid, "valid" if valid else "invalid", f"before ran={before['ran']} failed={len(before['failed'])}  after ran={after['ran']} failed={len(after['failed'])}"))
        print(log[-1], flush=True)
        if not valid:
            if not before["failed"] and before["ran"] == 0:
                print("   ", before["tail"][-300:].replace("\n", " | "), flush=True)
            continue
        tasks.append(dict(
            id=iid, family="D", repo_name=repo, base=str(base), env=str(env), test_cmd=cmd, f2p=f2p,
            issue=r["problem_statement"], category=r["category"], gold_files=sfiles, test_files=tfiles,
            gold_patch_bytes=len(r["patch"]), verified=dict(before=dict(ran=before["ran"], failed=len(before["failed"])), after=dict(ran=after["ran"], failed=0)),
        ))
        if a.limit and len(tasks) >= a.limit:
            break
    AB.mkdir(parents=True, exist_ok=True)
    json.dump(tasks, open(AB / "tasks.json", "w"), indent=1)
    print(len(tasks), "valid tasks written to", AB / "tasks.json")
    for row in log:
        if row[1] != "valid":
            print("  skipped:", row)


if __name__ == "__main__":
    main()
