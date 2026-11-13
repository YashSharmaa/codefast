# Benchmark datasets for LLM-free localisation

Task text in, ranked files and functions out. This directory holds the datasets. The loaders and the
tree-preparation script are in `bench/scripts/datasets/` (see its `README.md`).

Everything here was fetched on 2026-11-12, anonymously (no tokens, no `gh`, a generic User-Agent),
at pinned revisions. The only downloaded code that was run is ripwire's `r4_score.py`, read in full
first. It only aggregates result files committed next to it. repowise's table was recomputed from its
committed graded files with a few lines of our own. No tool under test was run.

| Dataset | Directory | Tasks | Gold | Licence of the data | Details |
|---|---|---|---|---|---|
| LinuxFLBench (arXiv 2505.19489) | `linuxflbench/` | 250 | 1 file per task, 472 function names | MIT | `linuxflbench/README.md` |
| Loc-Bench V1 (arXiv 2503.09089) | `locbench/` | 560 (ripwire round-4 slice: 60) | 676 files, 1,149 functions | none declared on the dataset card | `locbench/README.md` |
| ContextBench (arXiv 2602.05892) | `contextbench/` | 1,136 (verified: 500, repowise sealed: 42) | line spans in 4,548 files, no function gold | Apache-2.0 (GitHub repository) | `contextbench/README.md` |
| Live-kBench-2512 (arXiv 2602.02690) | `live-kbench/` | 534 | 817 files, 1,745 functions | MIT | `live-kbench/README.md` |

## Reproduce

```
bench/scripts/datasets/fetch_datasets.sh                 # all four, pinned, then checks SHA256SUMS
bench/scripts/datasets/fetch_datasets.sh locbench        # or one at a time
( cd bench/data && shasum -a 256 -c SHA256SUMS --quiet ) # 85 files
```

Tested on 2026-11-12 from an empty directory for `locbench` and `linuxflbench` (all 30 of their
checksums matched, including ripwire's frozen 560-row snapshot). The `contextbench` and `live-kbench`
branches use the same two helper functions and were not re-run from empty.

Exact commands used for the first fetch (what `fetch_datasets.sh` automates):

```
git clone --depth 1 https://github.com/FudanSELab/LinuxFLBench.git linuxflbench/upstream                          # de65c3958f264137835eb23cacf3591fb89fc09b
git clone --depth 1 https://github.com/EuniAI/ContextBench.git contextbench/upstream                              # 1436c28a8eb95496da4ea69ad458b9f8a8eb7d61
git clone --depth 1 https://github.com/repowise-dev/repowise-bench.git contextbench/repowise-bench                # 0fb3c03aab35fb67dc72540fd6a70e2c2c599878
git clone --depth 1 --filter=blob:none --no-checkout https://github.com/repowise-dev/repowise.git contextbench/repowise   # 8bed062714b896a8d50e9567d101152654339515
  git -C contextbench/repowise sparse-checkout set --no-cone /docs/BENCHMARKS.md /LICENSE && git -C contextbench/repowise checkout
git clone --depth 1 --filter=blob:none --no-checkout https://github.com/redhat-et/ripwire.git locbench/ripwire    # c4a991c238dbc5319c076eb39b9624e0a5db8414
  git -C locbench/ripwire sparse-checkout set --no-cone /bench/headtohead/ /bench/locbench/ /LICENSE /NOTICE && git -C locbench/ripwire checkout
git clone --depth 1 --filter=blob:none --no-checkout https://github.com/gersteinlab/LocAgent.git locbench/locagent # 4935b557326c154bad8e8dcf3747cc8d32d1f387
  git -C locbench/locagent sparse-checkout set --no-cone /evaluation/eval_metric.py /LICENSE /util/benchmark/ /README.md && git -C locbench/locagent checkout
git clone --depth 1 https://github.com/ARiSE-Lab/live-kbench.git live-kbench/upstream-code                        # c9d1e6cd6cf9b87d2a37eb1189ad72cfe0753514
curl -fsSL -o <file> https://huggingface.co/datasets/czlll/Loc-Bench_V1/resolve/c44cf3b74e07ca642cec841b471a9939907c12a7/<file>        # README.md, data/test-00000-of-00001.parquet
curl -fsSL -o <file> https://huggingface.co/datasets/live-kbench/live-kbench/resolve/e42003c93fe2c7da335d18252cb11823931445f6/<file>   # README.md, datasets/*.parquet, bugs/*.parquet
uv run bench/scripts/datasets/extract_live_kbench.py                                                               # writes live-kbench/lkbench-2512.metadata.jsonl
```

(all git commands ran with `-c credential.helper=` and `GIT_TERMINAL_PROMPT=0`.)

`SHA256SUMS` covers the data files, the licence files, and every third-party harness and result file
that a reproduction depends on (85 entries). The `.gitignore` here keeps all third-party payloads out
of version control. Only the READMEs, `SHA256SUMS`, `.gitignore` and the two small derived lists
(`linuxflbench/functions_absent_at_version.json`, `live-kbench/lkbench-2512.functions_absent_at_parent.json`)
are meant to be committed.

## Kernel sources

| What | Where | State on 2026-11-12 |
|---|---|---|
| Full-history clone | `bench/repos/linux-full` | `git clone https://github.com/torvalds/linux.git`, complete (not shallow). HEAD `9d66f26fefacebfeed15de31c1123451a17ff119` (v7.4-rc2+141, 2026-11-11), 1,506,331 commits on HEAD, 953 tags from origin plus 20 stable tags fetched for LinuxFLBench (973), packs 6.30 GiB, 8.1 GB with its work tree. Took about 40 minutes at 2 to 4 MB/s. |
| Stable tags | fetched into `linux-full` on demand | `https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git` (mirror `https://github.com/gregkh/linux.git`). The 20 that LinuxFLBench needs are already there. |
| Pre-git kernels (before 2.6.11) | kernel.org tarballs, fetched only on request | `https://cdn.kernel.org/pub/linux/kernel/v2.4/`, `v2.5/`, `v2.6/`, file `linux-<version>.tar.xz`, checksums in `sha256sums.asc` of the same directory. Only 2.4.26 has been fetched. |

The work tree of `linux-full` shows 13 files as modified: they are the case-colliding netfilter and
litmus files on this case-insensitive volume, not edits. Use `linux-full` for git objects only.
The clone log is `bench/data/.linux-full-clone.log`.

## Example trees

`bench/scripts/datasets/prepare_tree.sh` made three trees per dataset (twelve in all), each verified
against its source. They are listed in the per-dataset READMEs.

| | Size |
|---|---|
| `bench/data/` | 1.1 GB (contextbench 710 MB, live-kbench 324 MB, linuxflbench 83 MB, locbench 28 MB) |
| `bench/repos/linux-full/` | 8.1 GB |
| `bench/repos/_trees/` | 6.5 GB (three Live-kBench kernel trees 1.6 GB each, v5.6.7 1.0 GB, v2.6.27-rc2 314 MB, v2.4.26 182 MB, six non-kernel trees 209 MB, git cache 62 MB, tarball 23 MB) |

## Cross-dataset cautions

- Kernel trees on this (case-insensitive) volume lose 12 to 22 files each. One Live-kBench gold file
  is among them. See "Case-insensitive file systems" in `bench/scripts/datasets/README.md`.
- Gold functions that the fix itself adds cannot be found in the pre-fix tree. Loc-Bench separates
  them (`added_functions`). LinuxFLBench and Live-kBench do not, so their loaders take `--findable-only`.
- ContextBench has no function-level gold. Its `repo` column is wrong for 88 rows (the loader uses `repo_url`).
- ripwire's "strict file@10" uses a wider gold than LocAgent's file gold on the same instances, and
  ripwire's published 243-instance held-out set is not the split its current code produces.
- The loaders report what the datasets say. They do not judge whether a task is fair or solvable.
