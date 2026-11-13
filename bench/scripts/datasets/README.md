# Dataset loaders and tree preparation

Everything here is data plumbing for the localisation benchmarks (task text in, ranked files and
functions out). There is no ranking code and nothing here runs a tool under test.

Data lives in `bench/data/<dataset>/` (see `bench/data/README.md` for sources, licences and checksums).
Source trees live in `bench/repos/_trees/`. The full-history kernel clone is `bench/repos/linux-full`.

Requirements: `uv` (0.13.5 used), `git` (2.56.0 used), `jq`, `curl`, `shasum`, `tar` (bsdtar), bash 3.2+.
The loaders are PEP 723 inline scripts for Python 3.12 or newer. `uv run <script>` installs what they need.

## Files

| File | Purpose |
|---|---|
| `fetch_datasets.sh` | Re-download all four datasets at the pinned revisions, then check `bench/data/SHA256SUMS`. |
| `extract_live_kbench.py` | Hugging Face parquet to `bench/data/live-kbench/lkbench-2512.metadata.jsonl` (534 bugs, metadata only). |
| `load_linuxflbench.py` | LinuxFLBench, 250 tasks. |
| `load_locbench.py` | Loc-Bench V1, 560 tasks, subsets for ripwire's split and its round-4 slice of 60. |
| `load_contextbench.py` | ContextBench, 1,136 tasks, `verified` (500, default) and repowise's sealed 42 / dev 70. |
| `load_live_kbench.py` | Live-kBench-2512, 534 tasks. |
| `prepare_tree.sh` | Materialise one clean source tree and print its path. |
| `check_kernel_gold.py` | Check kernel gold files and functions against `linux-full` without writing a tree. |

## Loader output

One JSON object per line on stdout, always with exactly these keys in this order:

```json
{"id": "...", "repo": "...", "commit_or_version": "...", "query_text": "...", "gold_files": ["..."], "gold_functions": ["path:name"]}
```

- `repo` is `linux` for the two kernel datasets and GitHub `owner/name` otherwise.
- `commit_or_version` is a 40-hex commit, except for LinuxFLBench where it is a kernel version such as `5.6.7`.
- `gold_functions` entries are `"<file>:<name>"`. `<name>` is `Class.method` for Python methods.
  ContextBench has none (its gold is line spans. Pass `--extra` for them).
- Common flags: `--count`, `--ids a,b`, `--limit N`, `--extra` (adds an `"extra"` object with
  dataset-specific fields such as the gold patch, never needed for scoring).
- `load_linuxflbench.py` and `load_live_kbench.py` take `--findable-only`, which drops gold functions
  that do not exist in the tree the task is run on (functions the fix adds). The lists were computed
  with `check_kernel_gold.py` and are stored next to the data.
- `load_locbench.py --subset ripwire-r4-60` switches `query_text` to ripwire's 1,200-character
  normalised query and `gold_files` to ripwire's primary files, so that scoring "all gold in the top
  10" on its output is ripwire's strict file@10. `--gold` and `--query` override both.

Counts verified on the real data, 2026-11-12:

| Command | Tasks |
|---|---|
| `uv run load_linuxflbench.py --count` | 250 |
| `uv run load_locbench.py --count` | 560 |
| `uv run load_locbench.py --subset ripwire-r4-60 --count` | 60 |
| `uv run load_locbench.py --subset ripwire-heldout --count` | 306 |
| `uv run load_locbench.py --subset ripwire-heldout-243-legacy --count` | 243 |
| `uv run load_contextbench.py --subset full --count` | 1,136 |
| `uv run load_contextbench.py --count` | 500 |
| `uv run load_contextbench.py --subset repowise-sealed42 --count` | 42 |
| `uv run load_live_kbench.py --count` | 534 |

## prepare_tree.sh

```
prepare_tree.sh linuxflbench 207835            # kernel 5.6.7, tag fetched from the stable tree on demand
prepare_tree.sh linuxflbench 2567              # kernel 2.4.26, kernel.org tarball (pre-git)
prepare_tree.sh live-kbench 7daff104e6f71e7dd9aa016f37e7efe0ffb63a6b
prepare_tree.sh locbench UXARRAY__uxarray-1117
prepare_tree.sh contextbench Multi-SWE-Bench__go__maintenance__bugfix__0498ad7f
prepare_tree.sh --repo linux --commit 6.1
prepare_tree.sh --repo django/django --commit 4e8121e8e42a24acc3565851c9ef50ca8322b15c
```

- stdout is the tree path and nothing else. Progress goes to stderr. Non-zero exit on any failure.
- Tree: `bench/repos/_trees/<repo with / as __>@<commit or vX.Y.Z>/`, no `.git` inside.
  Sidecar: `bench/repos/_trees/.meta/<same name>.json` (source, git tree id, file counts, verification).
- An existing tree is reused (`--verify` re-hashes it against git first, `--force` rebuilds it).
- Concurrent calls for the same tree wait on a lock. Fetches into `linux-full` are serialised.
- Kernel version to source:
  1. tag `v<version>` in `linux-full` (mainline releases and release candidates, `v2.6.11` is a tree-only tag).
  2. otherwise the tag is fetched from `https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git`
     (fallback `https://github.com/gregkh/linux.git`) into `linux-full`.
  3. versions before 2.6.11 are not in git. `https://cdn.kernel.org/pub/linux/kernel/v2.4|v2.5|v2.6/linux-<version>.tar.xz`
     is downloaded to `_trees/.tarballs/` only when such a tree is requested, and its sha256 is checked
     against `sha256sums.asc` from the same directory. The PGP signature on that file is not checked.
- Other repositories: a bare cache per repository in `_trees/.gitcache/`, `git fetch --depth 1 <url> <sha>`.
- Verification of git-sourced trees: a fresh index is read from the tree object, every file on disk is
  re-hashed (`update-index --refresh`, `diff-files`), and `ls-files --others` must be empty.
  Tarball trees: tarball sha256, and member count against files on disk.
- `--verify` also fails when a tree contains files that are not in git, so tools that write an index
  directory inside the tree make it fail. Keep indexes outside the tree, or rebuild with `--force`.
- Safety: git runs with no credential helper, no hooks, no LFS smudge and no submodule fetch.
  LFS files stay pointer files. Submodule directories stay empty (counted in the sidecar).

### Case-insensitive file systems

The default macOS volume folds case, and the kernel has pairs of files that differ only by case
(`net/netfilter/xt_DSCP.c` and `xt_dscp.c`, and so on). One file of each pair is lost when a kernel
tree is written there. `prepare_tree.sh` detects this, lists the colliding paths in the sidecar
(`case_colliding_paths`) and prints a warning. `check_kernel_gold.py` reports which tasks have a
gold file among them.

For exact kernel trees, put the trees on a case-sensitive volume and point `CODEFAST_BENCH_TREES` at it:

```
hdiutil create -size 300g -type SPARSEBUNDLE -fs "Case-sensitive APFS" -volname codefast-trees ~/codefast-trees.sparsebundle
hdiutil attach ~/codefast-trees.sparsebundle          # mounts at /Volumes/codefast-trees, no sudo
export CODEFAST_BENCH_TREES=/Volumes/codefast-trees
```

This was not done here. The example trees are on the default (case-insensitive) volume.

## Environment variables

| Variable | Default | Used by |
|---|---|---|
| `CODEFAST_BENCH_TREES` | `bench/repos/_trees` | `prepare_tree.sh` |
| `CODEFAST_LINUX_FULL` | `bench/repos/linux-full` | `prepare_tree.sh` (`check_kernel_gold.py` takes `--linux`) |
| `KERNEL_TARBALL_BASE` | `https://cdn.kernel.org/pub/linux/kernel` | `prepare_tree.sh` |
| `CODEFAST_BENCH_DATA` | `bench/data` | `fetch_datasets.sh` (the loaders take explicit path flags instead) |
