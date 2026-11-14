# Benchmarks

Everything needed to reproduce the numbers in the top level README. Nothing here calls a
language model except the two agent experiments in `tier2`, which are clearly marked.

## Layout

| Directory | What it holds |
|---|---|
| `k3/` | 72 verified questions about the Linux kernel, with gold answers, and the runner that compares codefast with a best-case ripgrep workflow |
| `oracles/` | Call graphs produced by real compilers and type checkers for redis (C), gitleaks and syft (Go) and zod (TypeScript) |
| `tier1/` | Localisation without a model: task text in, ranked files and functions out |
| `tier2/` | Agent experiments: real headless sessions with and without the plugin (`live.py`), and an earlier pilot with sub-agents |
| `codex/` | The plugin inside Codex, checked with the real `codex` binary and a stand-in for the model |
| `acb/` | codefast as one more arm of a published agent benchmark (agent-context-bench: 26 graded questions about apache/superset) |
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

## Real sessions (`tier2/live.py`)

This part and the pilot after it use a language model, and this one spends real usage of a
Claude Code login. Each task is run in a headless session (`claude -p`) twice: with nothing
loaded, and with the plugin as `claude plugin install` gives it. The prompt is the same in both
and does not mention the tool.

```
export CLAUDE_CONFIG_DIR=$HOME/.claude        # the login to use
export CODEFAST_BIN=$PWD/target/release/codefast
uv run bench/tier2/make_n_tasks.py            # who-calls questions with a compiler-checked answer
uv run bench/tier2/live.py run --tag T --n 13 --l 3 --k 18 --d 12 --r 8 --arms control,plugin --model haiku --jobs 2
uv run bench/tier2/live.py score --tag T --out bench/results/live-T.jsonl
uv run bench/tier2/live.py show --tag T --id N-Collection-Add --arm plugin   # one session, step by step
```

The tasks held out until a build is final are listed in `bench/tier2/tasks.json` and run by
name:

```
uv run bench/tier2/make_n_tasks.py --repo go-git --repo-name go-git/go-git --out n2
uv run bench/tier2/make_n_tasks.py --repo zod --repo-name colinhacks/zod --out nz --functions
uv run bench/tier2/make_r_tasks.py --repo go-git --repo-name go-git/go-git --out r2 --vet "go vet ./..."
uv run bench/tier2/make_e_tasks.py           # from an error message to the function that raises it
uv run bench/tier2/make_i_tasks.py           # the bug-fix tasks without their tests
uv run bench/tier2/live.py run --tag H --held K,N,E,R,D,I,L --arms control,plugin --model haiku --jobs 2 --reps 2
```

Kernel bug reports are a family of their own (F), set up like the one published comparison of
repository maps for a coding agent on LinuxFLBench (arXiv 2611.07286): the report, a checkout of
the kernel version it was filed against, and up to ten files as the answer.

```
python3 -I bench/tier2/make_f_tasks.py          # 50 reports of the half the file ranking was never tuned on
python3 -I bench/tier2/make_f_tasks.py --dev    # 40 of the other half, to develop on
uv run --with tiktoken python -I bench/tier2/live.py run --tag F --held F --arms control,plugin --reps 2 --jobs 2
uv run --with tiktoken python -I bench/tier2/live.py score --tag F --out rows.jsonl --quiet
python3 -I bench/tier2/score_f.py rows.jsonl    # recall at 1, 5 and 10 and tokens, next to the published rows
```

`tier2/LIVE.md` is the protocol: the arms, the five kinds of task and how each answer is
checked, what is measured, and why the tables report a cost priced from the size of every
request instead of the billed cost. `--resume` continues a batch that was interrupted, and a
batch stops by itself when the login's usage window passes `--limit-stop`.

## Inside Codex, without a model (`codex`)

Whether the plugin works in Codex is a question about Codex, not about a model: does it fire
the hooks, does their text reach the model, do the tools answer inside its sandbox. So it is
checked with the real `codex` binary and no model. `wiring.py` makes a sandbox home under
`target/codex-wiring`, runs `codefast setup codex --hooks --apply` there, and runs `codex exec`
against `mock_model.py`, a local server that answers from a script and keeps every request
Codex sends. Nothing leaves the machine and your own `~/.codex` is not read.

```
python3 -I bench/codex/wiring.py --codefast target/release/codefast --out bench/results/codex-wiring.json
python3 -I bench/codex/wiring.py --model gpt-5       # another model entry: Codex offers its tools differently
```

It prints one line per check and the bytes of a request that are the plugin's. With
codex-cli 0.164.0 and codefast 0.3.0 all 14 checks hold: the four hook events fire and their
text is in the next request, the MCP tools and the command line answer in the read-only
sandbox, and a session in a repository without an index gets one. What a model in Codex does
with the cards is not measured by this, and has not been measured: the sessions of
`tier2/live.py` are Claude Code sessions.

## A published agent benchmark, with codefast as one more arm (`acb`)

agent-context-bench (Rybalchenko, 2026, https://github.com/Neko1313/agent-context-bench) asks
Claude Code 26 questions about apache/superset 6.0.0 and grades each answer against a key. An
arm is one MCP server and nothing else: the built-in tools are denied and the system prompt
says the tools must be used. Its repository has the tasks, the harness and every one of its
936 runs, for a file system server (grep and read), graphlens, Serena and CodeGraph.

`run_acb.py` runs the same protocol with codefast's MCP server as the arm. The command line,
the system prompt, the list of denied tools and the limit of turns are read from the published
harness at run time, and its tasks are not copied into this repository. `score` prints the
rows next to the published rows of the same model.

```
python3 -I bench/acb/run_acb.py fetch      # the benchmark's repository and superset 6.0.0
python3 -I bench/acb/run_acb.py run --arm codefast --out bench/results/acb-codefast-haiku.jsonl
python3 -I bench/acb/run_acb.py score bench/results/acb-codefast-haiku.jsonl
```

This spends model usage: 78 sessions of Haiku 4.5. `--arm codegraph` runs the published
CodeGraph arm again with the version installed under `bench/tools`, to see how far figures
from another machine and another month can be set next to figures from here.

## The rename command, judged by the compiler (`tier2/rename_*.py`)

No model is involved. `rename_offline.py` applies `codefast rename` to a fresh copy of syft for
each of the eleven rename tasks of the live benchmark and runs the task's own check (`go build`
and `go vet`). `rename_sample.py` does the same for a random sample of the types, struct fields
and methods of a Go repository, and counts a rename as passed when every complaint of the
compiler is on a line the command itself listed as not changed. What the compiler says about
the repository before any rename is subtracted.

```
python3 bench/tier2/rename_offline.py
python3 bench/tier2/rename_offline.py --tasks r2 --out bench/results/rename-offline-go-git.jsonl
python3 bench/tier2/rename_sample.py --repo syft --per-kind 10
python3 bench/tier2/rename_sample.py --repo go-git --per-kind 8 --out bench/results/rename-sample-go-git.jsonl
```

## Agent pilot (`tier2`)

An earlier and smaller experiment: one seed, tens of tasks, sub-agents of one harness, with
the guide pasted into the prompt.

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
