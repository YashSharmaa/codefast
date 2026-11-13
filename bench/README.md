# Benchmarks

Everything needed to reproduce the numbers in the top level README. Nothing here calls a
language model except the agent pilot in `tier2`, which is clearly marked.

## Layout

| Directory | What it holds |
|---|---|
| `k3/` | 72 verified questions about the Linux kernel, with gold answers, and the runner that compares codefast with a best-case ripgrep workflow |
| `oracles/` | Call graphs produced by real compilers and type checkers for redis (C), gitleaks and syft (Go) and zod (TypeScript) |
| `tier1/` | Localisation without a model: task text in, ranked files and functions out |
| `tier2/` | A small paired agent experiment: the same agent, the same task, with and without codefast |
| `scripts/datasets/` | Loaders for the third party datasets, and the script that materialises a source tree at a given commit |
| `scripts/oracles/` | The programs that built the call graph oracles, and the scorer |
| `scripts/competitors/` | Index and query wrappers for other tools, run in a sandbox |
| `data/` | Third party datasets, fetched at pinned revisions (not stored in git) |
| `repos/` | Source trees under test (not stored in git) |
| `results/` | Raw results, one JSON object per line |

## Setup

```
cargo build --release
export CODEFAST_BIN=$PWD/target/release/codefast
git clone --depth 1 https://github.com/torvalds/linux.git bench/repos/linux
bench/scripts/datasets/fetch_datasets.sh
```

The scripts are Python with inline dependency headers. Run them with `uv run`.

## Index cost (`k0`)

```
/usr/bin/time -l codefast --root bench/repos/linux index
bench/scripts/competitors/index_ripwire.sh $PWD/bench/repos/linux $PWD/bench/results/k0/ripwire.linux.json
```

One script per tool builds a fresh index of the kernel and records the wall time, the peak
memory of the whole process tree and the size of the index on disk. `BENCH_JOBS` sets the
number of threads for the tools that take one. The codefast figure is the median of three
runs. The other tools were run once each, because the slowest takes over half an hour.

## Kernel questions (`k3`)

```
uv run bench/k3/run_k3.py --questions bench/k3/questions.verified.jsonl
```

Each question has a gold answer that an independent pass verified against the source tree.
The runner asks codefast with one command, and asks ripgrep with the commands a skilled user
would type, then scores both outputs with the same function and counts the tokens of the
output with the `o200k_base` tokenizer. The ripgrep arm is a best case: it is told which
macro defines a system call and which files to read.

```
uv run bench/k3/run_k3.py --arms ripwire,cscope,global,ctags,codebase-memory-mcp
```

The other tools are asked through `scripts/competitors/query_<tool>.sh` with the one command
that fits the kind of question: definition, callers, callees or text search. A question a
tool has no command for is recorded as unsupported and scores zero. Each tool needs its
index from the previous section first.

## Call graph accuracy (`oracles`)

```
codefast --root bench/repos/gitleaks export-edges > edges.jsonl
python3 bench/scripts/oracles/score.py --oracle bench/oracles/gitleaks --tool edges.jsonl --out score.json
```

The oracle for a repository lists every call edge the compiler or type checker resolves.
An edge the tool reports and the oracle contradicts counts against precision. An edge the
oracle has and the tool lacks counts against recall. `--repowise-compat` restricts scoring to
the population used in a published comparison, so that the two can be put side by side.
`export-edges --dynamic` adds the candidates of dynamic dispatch (overrides, interface
implementations, functions bound into a slot), marked as such.

## Localisation without a model (`tier1`)

```
uv run bench/tier1/run_locate.py --dataset linuxflbench
uv run bench/tier1/run_locate.py --dataset locbench --loader-args "--subset ripwire-r4-60"
uv run bench/tier1/run_locate.py --dataset locbench --loader-args "--subset ripwire-heldout-243-legacy"
uv run bench/tier1/run_locate.py --dataset contextbench --loader-args "--subset repowise-sealed42"
```

For every task the runner checks out the source tree at the commit before the fix, indexes it,
passes the issue text to `codefast locate`, and compares the ranked files with the files the
real fix touched. Every task is assigned to a development half or a held-out half by a hash of
its id. Ranking changes are judged on the development half only.

Run one repository at a time (`--jobs 1`) when several tasks share a repository, because the
tree preparation script fetches into one cache per repository.

## Agent pilot (`tier2`)

This is the only part that uses a language model. It is a pilot, not a full study: one seed,
tens of tasks, sub-agents of one harness.

Each task is run by the same agent in two or three arms. The control arm has shell tools only.
The tool arm also has the codefast command and its guide. The rule arm adds one paragraph of
project instructions that says to try codefast first. `score_ab.py` reads the transcripts and
reports, per arm: the score, the tokens processed, the number of requests and tool calls, and
how often the tool was used. Code-writing tasks are checked by running the project's own tests.

```
uv run bench/tier2/make_w_tasks.py            # build the code-writing tasks
uv run bench/tier2/score_ab.py TRANSCRIPT_DIR  # score a finished run
```

## Other tools

`scripts/competitors/` holds one index script and one query script per tool. They run every
tool with an empty environment and a throwaway home directory, because several of these tools
write agent configuration when they start. See `tools/README.md` for versions, checksums and
what each tool was able to answer.
