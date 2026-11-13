# codefast

Code intelligence for AI coding agents. One small binary indexes a repository, the whole Linux
kernel in seconds, and answers the questions an agent keeps asking in a single call and a few
hundred tokens:

* Where is this defined?
* Who calls it, including through callback tables and function pointers?
* What does it call?
* How does execution get from A to B?
* Which function prints this log message?
* Which files does this bug report concern?

It never calls a language model. It reads the code with tree-sitter, keeps a compact index on
disk, and notices your edits by itself.

It plugs into Claude Code, Codex, Gemini CLI, Cursor and other harnesses as a skill plus a
command line tool, as an MCP server, and with optional hooks.

## Why

An agent spends most of its tokens finding and reading code. Every grep and every file read is
another round trip that sends the whole conversation again. codefast answers a structural
question in one call, with the exact lines and locations, so the agent takes fewer turns and
reads less.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/YashSharmaa/codefast/main/install.sh | sh
codefast setup                  # shows what it would register, changes nothing
codefast setup --all --apply    # register with every harness found on this machine
```

The script puts one binary in `~/.local/bin` after checking its SHA-256. It uses neither sudo
nor your shell profile. `codefast setup` registers the skill and the MCP server through each
harness's own command or configuration file, and `codefast doctor` shows what is in place.

As a Claude Code plugin, which brings the skill, the command, the MCP server and the hooks:

```sh
claude plugin install codefast --marketplace YashSharmaa/codefast
```

Inside a Claude Code session the same is `/plugin install codefast --marketplace YashSharmaa/codefast`.

From source: `cargo install --git https://github.com/YashSharmaa/codefast`.

`docs/INSTALL.md` covers every harness and says for each one what was tested and how.

## Quick start

```
codefast search vfs_read            # start here: a symbol, a file, or free text
codefast callers ext4_file_read_iter
codefast path sys_read ext4_file_read_iter
codefast locate "paste a bug report or a stack trace here"
```

The index builds itself on first use and follows your edits. There is nothing to configure.

| You want | Run |
|---|---|
| Anything about a symbol, a file or free text | `codefast search NAME` |
| Where it is defined | `codefast find NAME` |
| One function or struct, or a file outline | `codefast show NAME`, `codefast show path/file.c` |
| Every use, grouped by kind | `codefast refs NAME` |
| Who writes a field | `codefast refs task_struct.on_rq --only write` |
| Who calls it | `codefast callers NAME` |
| What it calls | `codefast callees NAME` |
| Everything about a function before changing it | `codefast context NAME` |
| How execution gets from A to B | `codefast path A B` |
| Which function prints a message | `codefast grep -F "text of the message"` |
| Where to look for a bug report or stack trace | `codefast locate "text"` |
| A pasted stack trace, one frame per line | `codefast trace` |
| The API of an installed dependency | `codefast dep`, `codefast dep NAME SYMBOL` |

## While writing code

* The index follows your edits. A function written a second ago can be found, shown and
  traced, with nothing to rebuild.
* `codefast context NAME` is the reading done before an edit, in one answer: the function's
  body, the type it belongs to, the signatures of what it calls, the lines that call it, the
  tests that reach it and the command that runs them.
* `codefast show NAME` returns one function or type instead of a whole file, so the agent
  reads what it is about to change and nothing else.
* `codefast callers NAME` lists every call site with its source line, so a signature change
  can be carried out from one answer.
* `codefast dep NAME SYMBOL` looks up a signature inside an installed dependency (a Python
  virtual environment, `node_modules`, the Go module cache, the cargo registry), which saves
  reading the package or guessing its API.

## Results

All numbers below were measured on one machine (Apple M5 Max, 18 cores, 36 GB) in November 2026.
The scripts, the questions and the raw results are in `bench/`. A figure for another tool is
either measured here with the same scripts or that project's own published figure, and each
table says which.

### Index a huge repository

The Linux kernel: 97,242 files, 44 million lines, 6.3 million symbols, 23 million references.
All tools ran on the same machine in the same hour, each with as many threads as it would
take. The machine was also doing other work (load average 8 to 20 when each run started), so
read the times as a ranking. The codefast row is the median of three runs. The others are one
run each. Where a project publishes its own figure, it is in the last column.

| Tool | Index time | Peak memory | Index on disk | Published by the project |
|---|---:|---:|---:|---|
| **codefast** | **7.6 s** | 3.7 GB | **0.6 GB** | |
| ripwire 0.6.10 | 34 s | 11.5 GB | 3.4 GB | |
| GNU Global 6.7 | 43 s | 1.4 GB | 1.7 GB | |
| universal-ctags 6.2.1 | 48 s | 2.4 GB | 1.8 GB | |
| cscope 15.9 | 119 s | 3.9 GB | 2.7 GB | |
| codebase-memory-mcp 0.12.1 | 725 s | 11.2 GB | 15.8 GB | 3 min, 72 s in fast mode |

After the first build the index follows edits by itself: a query re-reads only the files
that changed. Other large trees on the same machine:

| Repository | Size | Index time |
|---|---|---:|
| LLVM | 48.5 million lines, 189,399 files | 9.6 s |
| Elasticsearch | 9.6 million lines | 3.1 s |
| Kubernetes | 7.4 million lines | 1.3 s |
| VS Code | 7.0 million lines | 1.5 s |
| Rust compiler | 6.3 million lines | 2.1 s |
| Django | 0.76 million lines | 0.2 s |

Writing out the whole call graph of Kubernetes (345,000 edges) takes 12 s, and of VS Code
(325,000 edges) 10 s.

### Answer 72 kernel questions in one call

`bench/k3` holds 72 questions about the kernel with verified answers: where something is
defined, signatures, callers, callees, which function sits in an ops table slot, who calls a
function that is only reached through a pointer, call paths, which function prints a log
line, and who writes a field. The ripgrep column is a best case, typed by someone who already
knows which macro defines a system call and which lines to read.

| | Correct | Tokens of tool output, median (mean) | Tool calls |
|---|---:|---:|---:|
| **codefast** | **0.95** | **148** (196) | **1.0** |
| ripgrep, best case | 0.89 | 384 (1,101) | 2.0 |
| ripwire 0.6.10 | 0.66 | 889 (3,473) | 1.1 |
| cscope 15.9 | 0.44 | 120 (1,370) | 1.1 |
| GNU Global 6.7 | 0.27 | 54 (1,252) | 1.1 |
| codebase-memory-mcp 0.12.1 | 0.26 | 158 (723) | 1.1 |
| universal-ctags 6.2.1 | 0.13 | 12 (56) | 1.1 |

The other tools were asked through their own command for each kind of question: definition,
callers, callees or text search. That is one direct query per question, not a tuned workflow,
so read those rows as what each tool gives out of the box. A question a tool has no command
for scores zero, which is most of the list for ctags (it only knows definitions).

### Call graphs checked against compilers

The call graph codefast reports was compared with what a compiler or type checker resolves
for the same code (`bench/oracles`). Precision is the share of reported edges the compiler
agrees with. Recall is the share of the compiler's direct call edges that were reported.

| Repository | Language | Precision | Recall |
|---|---|---:|---:|
| redis | C | 0.982 | 0.972 |
| gitleaks | Go | 1.000 | 1.000 |
| syft | Go | 1.000 | 0.998 |
| zod | TypeScript | 1.000 | 0.969 |

On the Go and TypeScript repositories no reported edge is contradicted by the compiler.

repowise publishes the same kind of comparison for these repositories. Scored on its edge
population, which also counts calls that only a type checker can resolve, codefast gets 1.000
precision and 0.982 recall on gitleaks (repowise 0.991 and 0.971), 1.000 and 0.581 on syft
(0.979 and 0.539), and 1.000 and 0.968 on zod (0.995 and 0.796). On redis repowise publishes
0.938 and 0.717.

Calls through interfaces are a separate list of candidates (`export-edges --dynamic`). With
them, recall over every edge in syft, dynamic ones included, is 0.885 at a precision of 0.883.

### Find the files for a bug report, without a model

The issue text goes in, a ranked list of files comes out. No model is involved. The other
columns are the figures each project or paper publishes for the same task lists.

| Benchmark | Measure | codefast | Best published |
|---|---|---:|---|
| LinuxFLBench, 250 kernel bugs | right file ranked first | 0.344 | 0.228 (BLUiR), 0.168 (BM25) |
| | right file in the top 10 | 0.608 | 0.404 (BLUiR), 0.396 (BM25) |
| Loc-Bench, 60 task slice | all gold files in the top 10 | 0.667 | 0.583 (ripwire) |
| Loc-Bench, 243 held-out tasks | all gold files in the top 10 | 0.733 | 0.609 (ripwire) |
| ContextBench, 42 sealed tasks | gold files covered by 20 files | 0.898 | 0.876 at 19 files (repowise) |

A query takes 0.1 to 0.5 s. Ranking changes were judged on a development half of each task
list. The figures above are for the full lists.

### With an agent in the loop

A small paired experiment: the same agent answers the same task with shell tools only, and
again with codefast available. One run per task and arm, sub-agents of a single harness, so
treat these as a pilot.

| Tasks | Model | Correct, without and with | Tokens processed | Requests per task |
|---|---|---|---:|---|
| 36 kernel navigation questions | Haiku 5.5 | 1.00 and 1.00 | 26% to 32% fewer | 4.3 down to 3.1 |
| 18 of the same questions | Sonnet 5.5 | 0.98 and 1.00 | 26% fewer | 3.2 down to 2.3 |
| 24 bug reports to localise | Haiku 5.5 | no clear difference | no saving | |
| 8 functions to write until tests pass | Sonnet 5.5 | 8 of 8 and 8 of 8 | no saving | |

What this says: codefast removes about a quarter to a third of the tokens and turns when the
question is about code structure, at the same accuracy. It did not save tokens on bug
localisation, where the agent still has to read and reason, or on small self-contained
writing tasks, where finding code is not the hard part.

## How it works

* **Facts, linked by name.** Each file is parsed alone into definitions, references and imports.
  Nothing in one file's facts points into another file, so a changed file is re-read alone and the
  index never needs a full rebuild after an edit.
* **Linking at query time.** A reference is tied to its definition when you ask: by scope,
  imports, package, receiver type, arity and include reachability. Types keep the package they
  were written with, so two types with the same name in different packages stay apart.
* **Callbacks are edges.** A function stored in an ops table or passed as a callback is linked
  to the places that call through that slot. This is how `callers` reaches a function that has
  no direct call site.
* **A word index over every text file.** It answers "which files mention this at all", makes
  text search fast, and lets an answer say what it does not cover (comments, strings, unparsed
  files).
* **Answers sized for a context window.** Plain text, grouped by file, `path:line` locations, a
  token budget, and an exact count of what was left out.

## Languages

C is the most mature, including the macro idioms of the Linux kernel. C++, Python, JavaScript,
TypeScript, Go, Rust and Java are supported. Other files are covered by the word index and text
search.

## Limits

* It reads syntax, not types. A call through a value whose type only a compiler knows
  (inferred types, generics, narrowing) is not tied to its target. The answer counts such call
  sites and `codefast refs NAME --loose` lists them.
* Macros that build code by pasting tokens can hide a definition or a call. The kernel's
  static calls and tables filled through a name-pasting macro are followed. Tables generated
  by including a list file several times (the LSM hooks, for one) are not.
* It does not run builds, so code that is generated at build time is not indexed.

## Reproducing the numbers

See `bench/README.md`. Each benchmark has one script. The compiler oracles, the kernel
questions and the task lists are in the repository. The third party datasets are fetched at
pinned revisions by `bench/scripts/datasets/fetch_datasets.sh`.

## Licence

MIT or Apache-2.0, at your option.
