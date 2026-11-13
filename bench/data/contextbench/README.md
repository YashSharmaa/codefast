# ContextBench, the verified subset, and repowise's sealed split of 42

Fetched 2026-11-12. Re-fetch everything here with `bench/scripts/datasets/fetch_datasets.sh contextbench`
(pinned revisions, anonymous, then checks `../SHA256SUMS`).

## What is here

| Directory | Source | Revision | Licence |
|---|---|---|---|
| `upstream/` | https://github.com/EuniAI/ContextBench (gold parquet files under `data/`, evaluator package `contextbench/`, docs) | commit `1436c28a8eb95496da4ea69ad458b9f8a8eb7d61` (2026-06-12) | Apache-2.0 (`upstream/LICENSE`) |
| `repowise-bench/` | https://github.com/repowise-dev/repowise-bench (the harness and every raw result of repowise's bake-off) | commit `0fb3c03aab35fb67dc72540fd6a70e2c2c599878` (2026-10-01) | No `LICENSE` file. Its `README.md` says "released under the Apache 2.0 license". |
| `repowise/` | https://github.com/repowise-dev/repowise, sparse checkout of `docs/BENCHMARKS.md` and `LICENSE` | commit `8bed062714b896a8d50e9567d101152654339515` (2026-11-12) | AGPL-3.0 (documentation only, no repowise code is here) |

Paper: arXiv 2602.05892. Leaderboard: https://contextbench.github.io/

The Hugging Face copy (https://huggingface.co/datasets/Contextbench/ContextBench, commit
`c2855792b006af41c67202d33883fb9d46362853`, no licence on the card) was not downloaded: the sha256 of
all six parquet files it lists equals the GitHub copies here (compared against the LFS object ids from
the Hugging Face API).

## The dataset

| File | Rows | sha256 |
|---|---|---|
| `upstream/data/full.parquet` | 1,136 | `2f56535bdc73eb8a68bf4ebb49789d8e9cd4f219ea60df6290b85278aee61ca8` |
| `upstream/data/contextbench_verified.parquet` | 500 | `e9dcfd504cbfb849ac815a79040c793d0d92f94eecc9b5a4ee3e1445a2f8a791` |

(`train` / `test` splits of both and the per-source CSV lists are in the same directory. See `../SHA256SUMS`.)

Columns: `instance_id`, `original_inst_id`, `repo`, `repo_url`, `language`, `base_commit`,
`gold_context` (JSON list of `{file, start_line, end_line, content}`), `patch`, `test_patch`,
`problem_statement`, `f2p`, `p2p`, `source`.

- full: python 512, javascript 153, typescript 119, go 104, c 68, rust 63, cpp 60, java 57.
  Sources: Multi-SWE-bench 461, SWE-bench Verified 313, SWE-PolyBench 219, SWE-bench Pro 143.
- verified: python 266, typescript 66, javascript 60, go 40, c 23, rust 20, java 15, cpp 10.
  All 500 verified ids are in full.
- Every `repo_url` is a GitHub URL and every `base_commit` is a 40-hex sha. All 1,136 base commits are distinct.

## Loader

```
uv run bench/scripts/datasets/load_contextbench.py --count                         # 500 (verified)
uv run bench/scripts/datasets/load_contextbench.py --subset full --count           # 1136
uv run bench/scripts/datasets/load_contextbench.py --subset repowise-sealed42 --extra
```

Verified counts on 2026-11-12:

| `--subset` | tasks | repos | gold files | tasks with more than one gold file |
|---|---|---|---|---|
| `full` | 1,136 (1,132 with `--drop-unusable`) | 67 | 4,548 | 803 |
| `verified` | 500 | 59 | 1,746 | 298 |
| `repowise-sealed42` | 42 | 2 | 87 | 20 |
| `repowise-dev70` | 70 | 2 | 174 | 35 |
| `repowise-112` | 112 | 2 | 261 | 55 |

`gold_functions` is always empty: ContextBench gold is line spans. `--extra` adds
`gold_spans` (`{file, start_line, end_line}`), `language`, `source`, `original_inst_id`, `repo_url`.

## Scoring, as upstream defines it

`python -m contextbench.evaluate --gold data/contextbench_verified.parquet --pred pred.jsonl --cache <dir> --out results.jsonl`
(run from `upstream/`. Dependencies in `upstream/requirements.txt`. It clones each repo at `base_commit`
into `--cache` and uses tree-sitter. Network and git, no Docker).

Per instance and per granularity (file, symbol, span, line) it reports
`coverage = |pred ∩ gold| / |gold|` and `precision = |pred ∩ gold| / |pred|`, plus trajectory metrics.
A retrieval-only prediction must be wrapped, one JSON object per line:

```json
{"instance_id": "...", "traj_data": {"pred_files": ["a.py"], "pred_spans": {}, "pred_steps": [{"files": ["a.py"], "spans": {}}]}}
```

Without the `traj_data` wrapper the evaluator loads the line and scores it as `no_context_extracted`
(found by repowise. See `to_pred` in `repowise-bench/results/bakeoff_2026_08/rung8/canary_allarms.py`).
`pred_spans` is a dict `{path: [{"start": int, "end": int}]}`.

## repowise's sealed split and harness

- Split file: `repowise-bench/results/bakeoff_2026_08/dev_test_split.json` (created 2026-08-01),
  generator `make_split.py` in the same directory. All `django/django` (80) and `cli/cli` (32)
  instances of the verified subset, stratified by (repo, single-file or multi-file gold),
  seed 20260801, 62.5% dev: 70 dev, 42 sealed ("test").
- Sealed 42: 30 django (python, source Verified) and 12 cli/cli (go, source Multi), 42 distinct base
  commits, 22 single-file and 20 multi-file tasks, 87 gold files.
- Checked: `dataset_sha256` in the split file equals the sha256 of `upstream/data/contextbench_verified.parquet`.
  Re-running the split logic on that parquet reproduces both id lists exactly.
- Harness: `repowise-bench/results/bakeoff_2026_08/rung8/rung8_runner.py` (parallel, resumable),
  which imports `rung8/canary_allarms.py` (arm launch, query, prediction shape, grader call) and
  `rung5/retrieval_probe_multiarm.py` (per-tool path extractors). Arm definitions: `configs/arms.yaml`.
  Per-tool setup notes: `head-to-head/arms/*.md`. Reproduction notes: `repro/README.md`.
  Description of the published table: `repowise/docs/BENCHMARKS.md` (section "Pointing the agent at the right files")
  and `repowise-bench/README.md` section 1.
- Query: the instance's `problem_statement`, unmodified. Prediction: the tool's ranked file list,
  whole (no top-k cut), in the `traj_data` wrapper, with no spans. Grading: the upstream evaluator
  above against `contextbench_verified.parquet`.
- Published numbers are the mean over instances of `final.file.coverage` and `final.file.precision`,
  and the mean `final.file.pred_size` ("files served").

**Reproduction check done here** (no tool was run. Means recomputed from their committed graded files
in `rung8/`):

| Arm | File | n | File coverage | Precision | Files served |
|---|---|---|---|---|---|
| repowise `get_answer` | `graded__test-fix2__repowise.jsonl` | 42 | 0.876 | 0.087 | 19.2 |
| repowise `search_codebase` | `graded__test-fix2__repowise-search.jsonl` | 42 | 0.742 | 0.168 | 8.2 |
| CodeGraph | `graded__test-emb1__codegraph.jsonl` | 42 | 0.610 | 0.093 | 14.0 |
| Graphify | `graded__test-emb1__graphify.jsonl` | 42 | 0.546 | 0.033 | 34.5 |
| code-review-graph | `graded__test-emb1__crg.jsonl` | 42 | 0.445 | 0.240 | 5.4 |
| cocoindex | `graded__cocoindex-sealed42__cocoindex.jsonl` | 41 | 0.361 | 0.092 | 7.1 |

These equal the published table. The matching `pred__<tag>__<arm>.jsonl` files hold each arm's ranked
file lists, so an extra arm can be graded on the same 42 instances and compared instance by instance.

To add an arm without their runner: write one `traj_data` line per sealed instance, run the upstream
evaluator on it, and average `final.file.*`. To add an arm inside their runner, note that it was
developed on Windows: `canary_allarms.py` expects the parquet at `<repo>/data/contextbench/contextbench_verified.parquet`,
the evaluator at `<repo>/external/ContextBench` with a venv at `.venv-grader/Scripts/python.exe`, and
staged trees under `~/Desktop/bakeoff` (override with `BAKEOFF_TREES`). None of those paths exist in a
fresh clone on macOS.

## Problems found

1. **The `repo` column is wrong for 88 rows of full (34 of verified).** It is cut at the first hyphen
   of the repository name: `mui/material` for `mui/material-ui` (47 rows), `fasterxml/jackson` for
   `jackson-databind` (21), `jackson-core` (6) and `jackson-dataformat-xml` (4), `grpc/grpc` for
   `grpc-go` (5), `anuraghazra/github` for `github-readme-stats` (3), `zeromicro/go` for `go-zero` (2).
   `repo_url` is right. The loader derives `repo` from `repo_url`, which gives 67 repositories in full
   (the paper says 66, the count of the broken column) and 59 in verified.
2. full has 1 row with a null `problem_statement` and 3 rows with an empty `gold_context`
   (`--drop-unusable` removes these 4). verified has neither.
3. A span's `content` can run one line past its `[start_line, end_line]` range. Seen on 1 of 27 spans
   checked (`django/db/migrations/state.py` 455-488 in `SWE-Bench-Verified__python__maintenance__bugfix__0006beec`
   carries 35 lines of content). The evaluator scores the line range.
4. Gold is not function-level, so this dataset has no `gold_functions`. Symbol-level scoring exists
   only inside the upstream evaluator.
5. repowise's sealed split covers two repositories (django, cli) and two languages. The first sealed run
   (`test-emb1`) scored repowise `get_answer` at 0.597 on 40 valid rows. The published 0.876 is the
   later `test-fix2` run of the same sealed instances. Both sets of graded files are committed.
6. The Hugging Face card declares no licence. The GitHub repository's Apache-2.0 is the only licence
   statement for the gold data. repowise-bench has no licence file.

## Example trees (materialised and verified 2026-11-12)

| Task | Tree | Files | Gold present |
|---|---|---|---|
| `Multi-SWE-Bench__go__maintenance__bugfix__0498ad7f` (sealed, cli/cli) | `bench/repos/_trees/cli__cli@bdce08a793abe4744c02d537e37c9ba63601a849` | 344 | 1 of 1 files, 3 of 3 span contents equal the file's lines |
| `SWE-Bench-Verified__python__maintenance__bugfix__0006beec` (sealed, django) | `bench/repos/_trees/django__django@4e8121e8e42a24acc3565851c9ef50ca8322b15c` | 6,484 | 1 of 1, 3 of 4 span contents equal (the fourth is problem 3) |
| `Multi-SWE-Bench__c__maintenance__bugfix__a47dfbbf` (verified, facebook/zstd, C) | `bench/repos/_trees/facebook__zstd@6b4a3e019f8eeb3423065f7b24d790358e8cbc59` | 483 | 5 of 5, 20 of 20 span contents equal |
