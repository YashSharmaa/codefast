# /// script
# requires-python = ">=3.12"
# dependencies = ["pyarrow", "pandas"]
# ///
"""Agent experiment, family I: fix a reported bug with nothing but the report.

The tasks are those of family D (`make_d_tasks.py`), with what makes D easy taken away: the
tests of the fix are not in the tree and the prompt does not name them. The agent gets the issue
text and has to find the place itself. When the session is over, the tests that came with the
real fix are copied into the tree and run.

For every task of `target/ab/d/tasks.json` this writes `target/ab/i/<id>/base` (the tree before
the fix, without its tests) and a row in `target/ab/i/tasks.json`.

usage: uv run bench/tier2/make_i_tasks.py
"""
import json, re, shutil, subprocess
from pathlib import Path

import pandas as pd

ROOT = Path(__file__).resolve().parents[2]
D = ROOT / "target/ab/d"
OUT = ROOT / "target/ab/i"
PARQUET = ROOT / "bench/data/locbench/hf-czlll-Loc-Bench_V1/data/test-00000-of-00001.parquet"


def main():
    df = pd.read_parquet(PARQUET).set_index("instance_id")
    tasks = []
    for t in json.load(open(D / "tasks.json")):
        iid = t["id"]
        r = df.loc[iid]
        base = OUT / iid / "base"
        if base.exists():
            shutil.rmtree(base)
        shutil.copytree(t["base"], base, symlinks=True, ignore=shutil.ignore_patterns(".git", ".codefast", "__pycache__"))
        p = subprocess.run(["patch", "-R", "-p1", "-s", "--no-backup-if-mismatch", "-f"], cwd=base, input=r["test_patch"], capture_output=True, text=True)
        if p.returncode != 0:
            print("  skipped", iid, (p.stdout + p.stderr)[-200:].replace("\n", " | "))
            shutil.rmtree(OUT / iid)
            continue
        # How this project's tests are run, without saying which ones matter here.
        if t["repo_name"] == "django/django":
            how = f"cd {{repo}}/tests && PYTHONPATH={{repo}} {t['env']}/bin/python runtests.py --parallel 1 MODULE (for example `forms_tests` or `forms_tests.tests.test_forms`)"
        else:
            how = f"cd {{repo}} && PYTHONPATH={{repo}} {t['env']}/bin/python -m pytest -q -p no:cacheprovider tests/FILE.py"
        gold_functions = [str(f) for f in r["edit_functions"]]
        tasks.append(dict(
            id="I-" + iid, family="I", repo_name=t["repo_name"], base=str(base), tests_from=t["base"], env=t["env"], test_cmd=t["test_cmd"], how=how,
            test_files=t["test_files"], f2p=t["f2p"], issue=t["issue"], gold_files=t["gold_files"], gold_functions=gold_functions,
            names_a_gold_function=any(re.search(r"(?<![A-Za-z0-9_])" + re.escape(g.rsplit(":", 1)[-1].split(".")[-1]) + r"(?![A-Za-z0-9_])", t["issue"]) for g in gold_functions),
        ))
    OUT.mkdir(parents=True, exist_ok=True)
    json.dump(tasks, open(OUT / "tasks.json", "w"), indent=1)
    print(len(tasks), "tasks written to", OUT / "tasks.json")
    print(sum(t["names_a_gold_function"] for t in tasks), "of them name a function of the real fix in the issue text")


if __name__ == "__main__":
    main()
