<h1 align="center">codefast</h1>

<p align="center">
  codefast gives an AI coding agent the linked facts about a codebase in one call:<br>
  what a function is, who calls it, what it calls, what breaks when it changes and which tests cover it.
</p>

<p align="center">
  <a href="https://github.com/YashSharmaa/codefast/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/YashSharmaa/codefast/actions/workflows/ci.yml/badge.svg"></a>
  <a href="https://github.com/YashSharmaa/codefast/actions/workflows/install-check.yml"><img alt="install check" src="https://github.com/YashSharmaa/codefast/actions/workflows/install-check.yml/badge.svg"></a>
  <a href="https://github.com/YashSharmaa/codefast/releases/latest"><img alt="latest release" src="https://img.shields.io/github/v/release/YashSharmaa/codefast"></a>
  <a href="#licence"><img alt="licence: MIT or Apache-2.0" src="https://img.shields.io/badge/licence-MIT%20OR%20Apache--2.0-blue"></a>
  <br>
  <img alt="no LLM calls" src="https://img.shields.io/badge/LLM%20calls-none-blue">
  <img alt="8 languages" src="https://img.shields.io/badge/languages-8-blue">
  <img alt="works through a CLI, an MCP server and hooks" src="https://img.shields.io/badge/works%20through-CLI%20%2B%20MCP%20%2B%20hooks-blue">
</p>

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/how-it-works-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="docs/assets/how-it-works-light.svg">
    <img alt="How a question travels. The agent asks through the CLI, the MCP server or a hook. codefast looks the answer up in an index that follows edits and returns one answer with the code line of every call site. This replaces a grep followed by several file reads." src="docs/assets/how-it-works-light.svg" width="880">
  </picture>
</p>

An agent learns a codebase by grep and by reading files, and all of it stays in its context.
codefast answers from an index instead: one exact answer with the code lines, and no language
model. The aim is an agent that keeps less in its context, acts in fewer steps and is more
reliable when it debugs, changes code, writes new code or maintains a very large codebase.

Measured so far (the tables are under [Results](#results)):

- **Right in one call.** 0.96 out of 1 on 72 verified questions about the Linux kernel, at a
  median of 149 tokens per answer. Best-case ripgrep: 0.89, two calls, 384 tokens.
- **Fewer steps.** In a small pilot on kernel navigation questions, an agent with codefast
  processed 26% to 32% fewer tokens and made 3.1 requests per task instead of 4.3, at the same
  accuracy.
- **Fast and exact.** The whole Linux kernel indexes in 7.6 s. The call graph agrees with
  compilers at a precision of 0.982 to 1.000 on four repositories.
- **No gain, or not measured yet.** No saving on bug localisation or on small writing tasks in
  the same pilot. More reliable agents on whole tasks: not measured yet.

## What it is for

| When the agent is | codefast answers |
|---|---|
| **Debugging** | Which files a bug report or stack trace points to (`codefast locate "<paste the report>"`), which function prints a message (`codefast grep -F "Busy inodes after unmount"`) and how execution gets from A to B (`codefast path sys_read ext4_file_read_iter`). |
| **Changing code** | Before the edit, in one answer: the body, its type, the signatures of what it calls, its call sites and its tests (`codefast context vfs_read`). Then every use the change touches (`codefast refs vfs_read`). |
| **Writing new code** | One function or struct to build on instead of a whole file (`codefast show vfs_read`), and the API of an installed dependency (`codefast dep requests Session`). |
| **Working in a very large codebase** | Where a symbol is defined and who reaches it, also through ops tables and callbacks (`codefast search vfs_read`, `codefast callers ext4_file_read_iter`). |

## Get started

**1. Install the binary** on macOS or Linux. It goes to `~/.local/bin` after a SHA-256 check,
without sudo.

```sh
curl -fsSL https://raw.githubusercontent.com/YashSharmaa/codefast/main/install.sh | sh
```

**2. Register it with your harness.**

| Harness | One command |
|---|---|
| Claude Code | `claude plugin install codefast --marketplace YashSharmaa/codefast` |
| Codex | `codefast setup codex --hooks --apply` |
| Cursor | `codefast setup cursor --apply` |
| Gemini CLI | `gemini extensions install https://github.com/YashSharmaa/codefast --auto-update` |
| Any other harness | `codefast setup --all --apply` |

`codefast setup` alone prints the plan and changes nothing, and `codefast doctor` shows what is in
place. Without any setup an agent can still run `codefast` in its shell, and `codefast mcp` is a
stdio MCP server. The Claude Code plugin and the Codex command also register hooks, which add the
answer next to the agent's own grep.

**3. Ask.** The index builds itself on first use and follows your edits.

```sh
codefast search vfs_read                    # where it is defined, its source, who calls it
codefast context vfs_read                   # all the reading before an edit, in one answer
codefast callers ext4_file_read_iter        # every call site, also through ops tables
codefast locate "paste a bug report here"   # the files to open, ranked
```

<details>
<summary>All commands</summary>

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

`codefast dep` looks inside a Python virtual environment, `node_modules`, the Go module cache and
the cargo registry.

</details>

[`docs/INSTALL.md`](docs/INSTALL.md) covers every harness and building from source, and says for
each harness what was tested and how.

## Built on what the research measured

Each design choice answers a published measurement of how coding agents spend their tokens. The
last column is our own number, or says that we have none yet.

| What studies of coding agents found | What codefast does about it | What we measured |
|---|---|---|
| **Reading dominates.** 76.1% of an agent's tokens on SWE-bench Verified went to read operations ([SWE-Pruner](https://arxiv.org/abs/2601.16746)), and agents viewed 7.8 to 9.5 files per task where 1.4 mattered ([Coherence Collapse](https://arxiv.org/abs/2603.24631)). | `show` returns one function instead of a file. `context` returns the body, its type, the signatures of what it calls, its call sites and its tests in one answer. | A median answer of 149 tokens on 72 kernel questions (best-case ripgrep 384). Files opened per task: not measured yet. |
| **A location without the code costs more reads.** When a language server tool returned source lines with its locations, its score on multi-file renames rose from 0.67 to 0.83 and follow-up reads fell from 15.2 to 3.2 ([arXiv 2608.13568](https://arxiv.org/abs/2608.13568), a small study). | Every reference and call site comes with its source line and the function it sits in. | 1.0 tool calls per kernel question (best-case ripgrep 2.0). |
| **Every extra turn sends the conversation again.** The median model call in 13 million GitHub Copilot sessions carried a 68K token prompt ([Agentic Coding in the Wild](https://arxiv.org/abs/2608.00101)). | One call answers a structural question. | In our pilot on 36 kernel navigation questions, requests per task fell from 4.3 to 3.1 and tokens processed by 26% to 32%, at the same accuracy. |
| **Text search loses accuracy across files.** On blast-radius tasks in a repository of 400k lines grep scored 0.71 at 12,596 tokens, structural tools 0.84 to 0.93 at 748 to 1,368 ([936 runs](https://dev.to/neko1313_4/how-much-does-context-cost-an-ai-coding-agent-grep-vs-graph-vs-lsp-measured-across-936-runs-33m8)). | `callers` follows ops tables, callbacks and function pointers. `path A B` returns the call chain. | 0.96 on 72 kernel questions (best-case ripgrep 0.89). |
| **Agents seldom reach for an extra tool.** Of 1,996,558 real tool calls 1.4% went to MCP tools and 0.4% to skills, about half to the shell ([SWE-chat](https://arxiv.org/abs/2604.20779)). A short index in `AGENTS.md` passed 100% of Vercel's evals, an instructed skill 79% ([Vercel](https://vercel.com/blog/agents-md-outperforms-skills-in-our-agent-evals)). | A shell command first, then a skill, an MCP server, a short rules block for `AGENTS.md` and hooks that add the answer next to the agent's own grep. | In our pilot, with the guide in the prompt and no hooks, the agent ran codefast in 32 of 36 navigation tasks. With only the skill or the hooks: not measured yet. |
| **A better file ranking alone does not save tokens.** An index with the right file in its top 5 for 81% of queries (grep 39%) saved none in a live trial ([InventiveHQ](https://inventivehq.com/blog/code-index-vs-grep-ai-coding-agents-tokens)). | `locate` ranks the files for a bug report without a model. We report it as a ranking, not as a saving. | The right file is ranked first for 0.344 of 250 kernel bugs (best published 0.228). Our pilot on 24 bug reports found no token saving. |

## Results

Measured on one machine (Apple M5 Max, 18 cores, 36 GB) in November 2026. A figure for another
tool is either measured here with the same scripts or that project's own published figure, and
each table says which. [`bench/README.md`](bench/README.md) has one script per benchmark, and the
raw results are in `bench/results/`.

### Index a huge repository

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/index-time-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="docs/assets/index-time-light.svg">
    <img alt="Bar chart of the seconds each tool takes to index the Linux kernel. codefast 7.6, ripwire 34, GNU Global 43, universal-ctags 48, cscope 119, codebase-memory-mcp 725." src="docs/assets/index-time-light.svg" width="880">
  </picture>
</p>

The Linux kernel: 97,242 files, 44 million lines, 6.3 million symbols, 23 million references.

| Tool | Index time | Peak memory | Index on disk | Published by the project |
|---|---:|---:|---:|---|
| **codefast** | **7.6 s** | 3.7 GB | **0.6 GB** | |
| ripwire 0.6.10 | 34 s | 11.5 GB | 3.4 GB | |
| GNU Global 6.7 | 43 s | 1.4 GB | 1.7 GB | |
| universal-ctags 6.2.1 | 48 s | 2.4 GB | 1.8 GB | |
| cscope 15.9 | 119 s | 3.9 GB | 2.7 GB | |
| codebase-memory-mcp 0.12.1 | 725 s | 11.2 GB | 15.8 GB | 3 min, 72 s in fast mode |

All tools ran in the same hour, each with as many threads as it would take. The machine was also
doing other work (load average 8 to 20 when each run started), so read the times as a ranking.
The codefast row is the median of three runs. The others are one run each.

<details>
<summary>Other large repositories on the same machine</summary>

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

</details>

### Answer 72 kernel questions in one call

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/kernel-questions-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="docs/assets/kernel-questions-light.svg">
    <img alt="Two bar charts over 72 verified kernel questions. Mean score for correct answers: codefast 0.96, ripgrep best case 0.89, ripwire 0.66, cscope 0.44, GNU Global 0.27, codebase-memory-mcp 0.26, universal-ctags 0.13. Median tokens per answer in the same order: 149, 384, 889, 120, 54, 158, 12." src="docs/assets/kernel-questions-light.svg" width="880">
  </picture>
</p>

[`bench/k3`](bench/k3) holds 72 questions about the kernel with verified answers: where something
is defined, signatures, callers, callees, which function sits in an ops table slot, who calls a
function that is only reached through a pointer, call paths, which function prints a log line, and
who writes a field.

| | Correct | Tokens of tool output, median (mean) | Tool calls |
|---|---:|---:|---:|
| **codefast** | **0.96** | **149** (202) | **1.0** |
| ripgrep, best case | 0.89 | 384 (1,101) | 2.0 |
| ripwire 0.6.10 | 0.66 | 889 (3,473) | 1.1 |
| cscope 15.9 | 0.44 | 120 (1,370) | 1.1 |
| GNU Global 6.7 | 0.27 | 54 (1,252) | 1.1 |
| codebase-memory-mcp 0.12.1 | 0.26 | 158 (723) | 1.1 |
| universal-ctags 6.2.1 | 0.13 | 12 (56) | 1.1 |

Correct is the mean score over the 72 questions, from 0 to 1. The ripgrep row is a best case,
typed by someone who already knows which macro defines a system call and which lines to read.
Each other tool got one direct query per question through its own command, not a tuned workflow,
so read those rows as what the tool gives out of the box. A tool with no command for a kind of
question is recorded as unsupported for it, which is most of the list for ctags (it only knows
definitions).

### Call graphs checked against compilers

The call graph codefast reports was compared with what a compiler or type checker resolves for the
same code ([`bench/oracles`](bench/oracles)). Precision is the share of reported edges the compiler
agrees with. Recall is the share of the compiler's direct call edges that were reported.

| Repository | Language | Precision | Recall |
|---|---|---:|---:|
| redis | C | 0.982 | 0.972 |
| gitleaks | Go | 1.000 | 1.000 |
| syft | Go | 1.000 | 0.998 |
| zod | TypeScript | 1.000 | 0.969 |

On the Go and TypeScript repositories no reported edge is contradicted by the compiler. Calls
through interfaces are a separate list of candidates (`export-edges --dynamic`). With them, recall
over every edge in syft, dynamic ones included, is 0.885 at a precision of 0.883.

<details>
<summary>Next to the scores repowise publishes</summary>

repowise publishes the same kind of comparison for these repositories. Here codefast is scored
on repowise's edge population, which also counts calls that only a type checker can resolve.

| Repository | codefast, precision and recall | repowise, as published |
|---|---|---|
| gitleaks | 1.000 and 0.982 | 0.991 and 0.971 |
| syft | 1.000 and 0.581 | 0.979 and 0.539 |
| zod | 1.000 and 0.968 | 0.995 and 0.796 |

On redis repowise publishes 0.938 and 0.717.

</details>

### Find the files for a bug report, without a model

The issue text goes in, a ranked list of files comes out. No model is involved. The last column
is the figure each project or paper publishes for the same task list.

| Benchmark | Measure | codefast | Best published |
|---|---|---:|---|
| LinuxFLBench, 250 kernel bugs | right file ranked first | 0.344 | 0.228 (BLUiR), 0.168 (BM25) |
| | right file in the top 10 | 0.608 | 0.404 (BLUiR), 0.396 (BM25) |
| Loc-Bench, 60 task slice | all gold files in the top 10 | 0.667 | 0.583 (ripwire) |
| Loc-Bench, 243 held-out tasks | all gold files in the top 10 | 0.733 | 0.609 (ripwire) |
| ContextBench, 42 sealed tasks | gold files covered by 20 files | 0.898 | 0.876 at 19 files (repowise) |

A query takes 0.1 to 0.5 s. Ranking changes were judged on a development half of each task list.
The figures above are for the full lists.

### With an agent in the loop

A small paired experiment: the same agent answers the same task with shell tools only, and again
with codefast available. One run per task and arm, sub-agents of a single harness, so treat these
as a pilot. [`bench/tier2/PROTOCOL.md`](bench/tier2/PROTOCOL.md) has the prompts and the limits.

| Tasks | Model | Correct, without and with | Tokens processed | Requests per task |
|---|---|---|---:|---|
| 36 kernel navigation questions | Haiku 5.5 | 1.00 and 1.00 | 26% to 32% fewer | 4.3 down to 3.1 |
| 18 of the same questions | Sonnet 5.5 | 0.98 and 1.00 | 26% fewer | 3.2 down to 2.3 |
| 24 bug reports to localise | Haiku 5.5 | no clear difference | no saving | |
| 8 functions to write until tests pass | Sonnet 5.5 | 8 of 8 and 8 of 8 | no saving | |

What this says: codefast removes about a quarter to a third of the tokens and turns when the
question is about code structure, at the same accuracy. It did not save tokens on bug localisation,
where the agent still has to read and reason, or on small self-contained writing tasks, where
finding code is not the hard part.

## How it works

* **Facts, linked by name.** tree-sitter parses each file alone into definitions, references and
  imports. Nothing in one file's facts points into another file, so a changed file is re-read
  alone and the index never needs a full rebuild after an edit.
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

C, C++, Python, JavaScript, TypeScript (including TSX), Go, Rust and Java each have a full
extractor, and they share one index. A project that mixes them, for example a TypeScript frontend
with a Python or Go backend, works without configuration, and nothing depends on a particular
framework. C is the most mature, including the macro idioms of the Linux kernel. Files in other
languages are covered by the word index and text search.

## Limits

* It reads syntax, not types. A call through a value whose type only a compiler knows (inferred
  types, generics, narrowing) is not tied to its target. The answer counts such call sites and
  `codefast refs NAME --loose` lists them.
* Macros that build code by pasting tokens can hide a definition or a call. The kernel's static
  calls and tables filled through a name-pasting macro are followed. Tables generated by including
  a list file several times (the LSM hooks, for one) are not.
* It does not run builds, so code that is generated at build time is not indexed.
* It does not save tokens everywhere. Our pilot found no saving on bug localisation or on small
  self-contained writing tasks, and the pilot is small: one run per task and arm.

## Contributing

The contributor guide is [`AGENTS.md`](AGENTS.md), with short notes for specific work in
[`docs/agents/`](docs/agents/).

## Licence

MIT or Apache-2.0, at your option. See [`LICENSE-MIT`](LICENSE-MIT) and
[`LICENSE-APACHE`](LICENSE-APACHE).
