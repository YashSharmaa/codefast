# Live sessions: protocol

The earlier pilot (`PROTOCOL.md`) ran sub-agents with the guide pasted into the prompt. This
experiment runs the real thing: headless Claude Code sessions (`claude -p`), with and without
the codefast plugin, in the state a user would have it. `live.py` runs and scores it.

## What a run is

One session per task and arm. The prompt is the same in every arm: nothing in it says that a
tool is installed. An arm differs only in what the harness loads.

| Arm | What is loaded |
|---|---|
| `control` | nothing |
| `plugin` | the plugin as `claude plugin install` gives it: hooks, the skill, the MCP server, the command on `PATH` |
| `lean` | the plugin without its MCP server |
| `nohooks` | the plugin with `CODEFAST_HOOKS=off` |

Every session runs with the project's settings only (no user plugins, no cloud connectors), in
bypass-permissions mode inside a private copy of the repository, with the web tools and
anything that outlives the session switched off. Tasks that change nothing (questions) share
one checkout and one index. The plugin arms start with an index that exists, which is the state
of a project from its second session on.

## Tasks

| Family | Task | Checked by |
|---|---|---|
| N | Which functions call this method? 13 methods of syft (Go) whose name several types share, so that a text search finds more than the answer | the call graph the Go type checker built (`bench/oracles/syft`): F1 of the callers named with their file, against all real callers and against the callers of the other types' methods of that name |
| L | Four N questions in one session | the same, per question |
| K | 18 questions about the Linux kernel from `bench/k3` | the verified answers of `bench/k3` |
| D | Make failing tests pass: issues of Django and sqlglot from Loc-Bench, at the commit before the fix, with the tests of the fix added | the tests of the fix pass and are unchanged |
| R | Rename one Go method everywhere, where other types have a method of the same name | `go build` and `go vet` pass, and no other definition was renamed |

`make_n_tasks.py`, `make_d_tasks.py` and `make_r_tasks.py` build them. None
of the builders uses codefast to choose a task or to judge an answer. A few tasks of D and R
(`DEV_IDS` in `live.py`) were used while the hooks were being designed and are never reported.

## What is measured

All of it comes from the session's own event stream (`--output-format stream-json`):

* requests, tool calls by tool, characters of tool output, hook events that added context
* tokens by kind, and the cost at list prices that the harness reports
* **warm cost**: the session priced from the size of each request alone, as if the cache held
  everything an earlier request had sent. Cache reads count 0.1, new tokens 1.25 and output 5
  times an input token. This is the figure the tables report.

Why not the billed cost: it depends on whether the harness's own prompt happened to be in the
cache when a run started. In one batch the same arm cost 30% more or less for that reason
alone. The warm cost is what a session costs for anyone who started another one within the
hour, and it is the same formula for every arm.

## The runs behind the table in the README

The table is put together from runs of one line of builds, all version 0.2.0, on Haiku 5.5:

* Bug fixes (D) and kernel questions (K): one batch of both arms.
* Caller questions (N), sessions of four (L) and the control arm of the renames (R): a second
  batch of both arms, run after the receivers of Go calls were typed further. The first batch
  had these at 0.49, 0.30 and 1.11 of the control.
* The plugin arm of the renames: a later run with `codefast rename`, after the card for a
  rename request was reworded once. With the first wording the agent ran the command in 4 of 8
  sessions (0.56 of the control), with the second in 8 of 8. The second wording was chosen
  after reading those eight sessions, so this one figure is tuned on its own tasks.

`bench/results/live-haiku.jsonl` holds every run of the table, and its `tag` field says which
batch a run is from. `bench/results/rename-offline.jsonl` is the check of the rename command
without a model (`rename_offline.py`), and `bench/results/by-name.jsonl` the count of matches
by name against the type checker.

## Limits

* One harness (Claude Code 2.1.323) and, unless a table says otherwise, one model (Haiku 5.5).
* One run per task and arm unless repetitions are stated. Single pairs vary by a factor of two
  in either direction. Read the totals.
* The kernel is code the model has seen in training, and its names are unique. A control agent
  answers such questions in three requests, and there is little left to save.
* The tasks of N were chosen for names that several types share. That is where structure
  matters most, and it is not every question an agent asks.
* There is no rename family for a language without a compiler. Two attempts to build one for
  Python (sqlglot) failed for want of a reference to check against: a refactoring library
  renamed the wrong places, and a rename traced from the running tests left 12 of 16
  candidates with new failing tests. The four that passed changed three or four lines each.
