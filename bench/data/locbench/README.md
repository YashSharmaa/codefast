# Loc-Bench V1, and ripwire's round-4 slice and harness

Fetched 2026-11-12. Re-fetch everything here with `bench/scripts/datasets/fetch_datasets.sh locbench`
(pinned revisions, anonymous, then checks `../SHA256SUMS`).

## What is here

| Directory | Source | Revision | Licence |
|---|---|---|---|
| `hf-czlll-Loc-Bench_V1/` | https://huggingface.co/datasets/czlll/Loc-Bench_V1 (`data/test-00000-of-00001.parquet`, `README.md`) | dataset commit `c44cf3b74e07ca642cec841b471a9939907c12a7`, last modified 2025-04-27 | **none declared** on the dataset card. The code repo (LocAgent) is Apache-2.0. Treat the rows as research-use data and do not redistribute them. |
| `ripwire-r4-slice/rows_czlll__Loc-Bench_V1_test_560.json` | the same 560 rows as served by `https://datasets-server.huggingface.co/rows`, paged 100 at a time and re-serialised with `json.dumps` exactly as ripwire's `run_locbench.py` does | sha256 `5bbcea4bff11396f38f8aca3e64d697a8ea1da2bc54d705da7f6e34886804c97`, equal to `rows_json_sha256` in ripwire's `bench/locbench/dataset.lock` | as above |
| `ripwire/` | https://github.com/redhat-et/ripwire, sparse checkout of `bench/headtohead/`, `bench/locbench/`, `LICENSE` | commit `c4a991c238dbc5319c076eb39b9624e0a5db8414` (2026-11-12) | Apache-2.0 |
| `locagent/` | https://github.com/gersteinlab/LocAgent, sparse checkout of `evaluation/eval_metric.py`, `util/benchmark/`, `README.md`, `LICENSE` | commit `4935b557326c154bad8e8dcf3747cc8d32d1f387` (2026-08-12) | Apache-2.0 |

Paper: LocAgent, arXiv 2503.09089.

Checked: the parquet and the JSON snapshot hold the same 560 rows, field for field (560 of 560 equal).
The parquet's sha256 (`8df0833c...`) equals the LFS object id Hugging Face reports for that revision.

## The dataset

560 instances, all Python, 165 repositories, 526 distinct (repo, base commit) pairs.
Categories: 242 bug reports, 150 feature requests, 139 performance issues, 29 security.

Fields: `repo`, `instance_id`, `base_commit`, `patch`, `test_patch`, `problem_statement`, `hints_text`,
`created_at` (ms), `labels`, `category`, `edit_functions` (`"path:func"` or `"path:Class.method"`),
`added_functions`, `edit_functions_length`, `__index_level_0__`.

Gold, as LocAgent scores it (`locagent/evaluation/eval_metric.py`, `cal_metrics_w_dataset`):

- file level: the file part of each `edit_functions` entry.
- module level: `file:FirstComponent`.
- function level: `file:Qualified.name`, with a trailing `.__init__` removed from both gold and prediction.
- `Acc@k` is strict: a task counts only when every gold location is in the top k.

Things to know:

- 115 instances list `added_functions` (functions that do not exist at `base_commit`). They are not gold.
- In 104 instances the fix patch touches files that `edit_functions` does not name (tests, docs, files
  where only new functions were added). LocAgent ignores those files, ripwire counts them (below).
- 28 (repo, commit) pairs are shared by more than one instance.

## Loader

```
uv run bench/scripts/datasets/load_locbench.py --count                           # 560
uv run bench/scripts/datasets/load_locbench.py --subset ripwire-r4-60 --extra   # the round-4 slice
```

Verified counts on 2026-11-12:

| `--subset` | tasks | repos | gold files (default gold) | gold functions |
|---|---|---|---|---|
| `all` | 560 | 165 | 676 | 1,149 |
| `ripwire-heldout` | 306 | 88 | 365 | 665 |
| `ripwire-train` | 254 | 77 | 311 | 484 |
| `ripwire-heldout-243-legacy` | 243 | 78 | 299 | 564 |
| `ripwire-r4-60` | 60 | 40 | 134 (ripwire primary files) | 197 |

## ripwire head-to-head round 4: what it is and how to join it

Files: `ripwire/bench/headtohead/r4-2026-08-06/` (`README.md`, `r4_worker.py`, `r4_score.py`,
`SCOREBOARD.md`, `SCOREBOARD-2026-08-08.md`, `results/`) and `ripwire/bench/locbench/run_locbench.py`
(the metric code the worker imports).

**The slice.** `results/r4_ripwire_for.json` lists the 60 instances with their gold and ranks. They are
the first 60 instances, in dataset order, of the held-out half of ripwire's repository-disjoint split
(`sha256("ripwire-a7-v2\0" + lowercase(repo))`, first byte below 128 is train). Checked: the loader's
partition function reproduces all 60, in order, with none skipped. 40 repos, 60 distinct base commits,
32 single-file and 28 multi-file tasks.

**The query.** `" ".join(problem_statement.split())[:1200]`, the same for every arm.
`--subset ripwire-r4-60` emits exactly this as `query_text`.

**The gold.** ripwire's `gold_files` are the `edit_functions` files plus every file in the fix patch
(`+++ b/` and `--- a/` lines). Its `primary_files` are the subset of those that ripwire's own index
contained. The published numbers use `primary_files`: 134 files, of which 107 are `.py`, 8 `.rs`,
6 `.c`, 4 `.h`, 4 `.md`, 2 `.tsx`, 2 `.json`, 1 `.ts`. This is a wider gold than LocAgent's (88 files
on the same 60 tasks. The two agree on 41 of 60 tasks). `--subset ripwire-r4-60` emits
`primary_files` as `gold_files`. `--gold locagent` and `--gold patch` give the other two definitions.

**The ranks** (`run_locbench.py` `file_ranks`, `first_hit`): the arm's output is reduced to a list of
distinct file paths in first-appearance order. For each gold file the rank is the 0-based index of the
first entry that equals it exactly, or that has the same basename when that basename is unique in the
arm's universe (`git ls-files` of the checkout for competitor arms). No cut-off is applied to the list.

- `file_worst` = the largest rank, or null if any gold file is absent. **strict file@10** is `file_worst < 10`.
- `file_first` = the smallest rank found. **any@10** is `file_first < 10`.

**The checkout.** `git init`, `git fetch --depth 1 origin <base_commit>`, `git checkout -f`,
`git clean -qfdx` (`run_locbench.py` `checkout`). `prepare_tree.sh locbench <id>` gives the same files
without the `.git` directory.

**Adding an arm.** `r4_worker.py` has one function pair per tool and a `score_block(files, inst,
universe, wall)` call that produces the JSONL row; `r4_score.py` aggregates `r4_<arm>*.jsonl`. A new arm
needs a line per instance of the form
`{"instance_id", "repo", "base_commit", "n_universe", "n_gold", "search": {"ranked": [...], "wall", "franks", "file_first", "file_worst", "all_file_worst"}}`
and an entry in `ARMS` / `arm_ranks` of the scorer. Two path mismatches to fix before running the
worker as shipped: it reads `results/ripwire_for.json` and `work/datasets/rows_czlll__Loc-Bench_V1_test_560.json`
relative to its own directory, while the committed files are `results/r4_ripwire_for.json` and (here)
`ripwire-r4-slice/rows_czlll__Loc-Bench_V1_test_560.json`.

**Reproduction check done here** (no tool was run, only their scorer on their committed results):
`python3 -I ripwire/bench/headtohead/r4-2026-08-06/r4_score.py` prints the table of `SCOREBOARD.md`
exactly (ripwire 58.3% / 85.0%, cbm 40.0% / 63.3%, repowise 33.3% / 53.3%, graphify 31.7% / 46.7%,
aider 18.3% / 35.0%). Recomputing strict@10 and any@10 for the cbm and graphify arms from their
recorded ranked lists and the loader's `gold_files` agrees with the recorded ranks on 60 of 60 instances.

## Problems found

1. **The committed results are the 2026-08-06 run, not the 2026-08-08 re-run the README quotes.**
   `SCOREBOARD-2026-08-08.md` (aider 20.0% / 35.0%, aider_noperson 10.0% / 25.0%, different wall
   times) says its per-instance files are in `bench-assets/r4/rerun-2026-08-08/`, which is not in the
   repository. Accuracy for ripwire, cbm, graphify and repowise is stated to be identical in both runs.
2. **"Held-out, N=243, 78 repos, 60.9%" cannot be regenerated from the current split code.** The salt
   in `run_locbench.py` today gives 306 held-out instances in 88 repos (and 254 train in 77). The 243
   ids exist only as a result file (`bench/locbench/results/r1_anchorhop/heldout_baseline_release.json`).
   The loader exposes them as `ripwire-heldout-243-legacy`. Only 28 of the round-4 60 are in that list.
   Later result files in the same repository (r3 onwards) use the 306 / 254 split.
3. ripwire's `primary_files` depend on what ripwire indexed, so the round-4 gold is not tool-neutral:
   a file ripwire does not parse is dropped from everyone's gold.
4. The ripwire arm's ranks come from ripwire's own symbol universe, the competitor arms' from
   `git ls-files`. The basename fallback therefore applies to slightly different universes.
5. No licence on the Hugging Face dataset card.

## Example trees (materialised and verified 2026-11-12)

| Task | Tree | Files | Gold present |
|---|---|---|---|
| `UXARRAY__uxarray-1117` | `bench/repos/_trees/UXARRAY__uxarray@fe4cae1311db7fb21187b505e06018334a015c48` | 309 | 2 of 2 files, 2 of 2 functions |
| `ultralytics__ultralytics-17810` | `bench/repos/_trees/ultralytics__ultralytics@d8c43874ae830a36d2adeac4a44a8ce5697e972c` | 674 (equals `n_universe` in ripwire's recorded cbm row) | 1 of 1, 1 of 1 |
| `django__django-18435` | `bench/repos/_trees/django__django@95827452571eb976c4f0d5e9ac46843948dd5fe6` | 6,826 | 1 of 1, 1 of 1 |
