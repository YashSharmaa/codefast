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
| `uses-count`, `uses-list` | the plugin, with one form of the line about a symbol's uses forced (see "What a card should say") |

Every session runs with the project's settings only (no user plugins, no cloud connectors), in
bypass-permissions mode inside a private copy of the repository, with the web tools and
anything that outlives the session switched off. Tasks that change nothing (questions) share
one checkout and one index. The plugin arms start with an index that exists, which is the state
of a project from its second session on.

## Tasks

| Family | Task | Checked by |
|---|---|---|
| N | Which functions call this method? Methods of a Go repository whose name several types share, so that a text search finds more than the answer | the call graph the Go type checker built (`bench/oracles`): F1 of the callers named with their file, against all real callers and against the callers of the other types' methods of that name |
| L | Four N questions in one session | the same, per question |
| E | A run ended with this error message: which function creates it, and which functions call that function? The message stands in one place of the source, written out in full | the function around that line and its callers in the type checker's call graph |
| K | Questions about the Linux kernel from `bench/k3` | the verified answers of `bench/k3` |
| D | Make failing tests pass: issues of Django and sqlglot from Loc-Bench, at the commit before the fix, with the tests of the fix added | the tests of the fix pass and are unchanged |
| I | The same issues without their tests: the issue text alone | the tests of the real fix, copied into the tree after the session, pass |
| R | Rename one Go method everywhere, where other types have a method of the same name | `go build` and `go vet` pass, and no other definition was renamed |

`make_n_tasks.py`, `make_e_tasks.py`, `make_d_tasks.py`, `make_i_tasks.py` and `make_r_tasks.py`
build them. None of the builders uses codefast to choose a task or to judge an answer.

## Development tasks and held-out tasks

Hooks are shaped by reading sessions, and a figure measured on the sessions that were read says
little. So the tasks are split.

* **Development tasks**: N, L and R on syft, 18 kernel questions and 12 bug-fix tasks. Every
  session of these may be read, and what the hooks say may be changed because of what was
  seen. The table "The tasks the hooks were developed on" in the README is from these. A few
  more (`DEV_IDS` in `live.py`) were used before any table existed and are never reported.
* **Held-out tasks** (`held_out` in `tasks.json`): tasks no session had been run on. N, L, R
  and half of E are on go-git, a repository nothing in codefast had been run on. The others are
  18 more kernel questions, the 15 bug-fix tasks not used before (as D and as I), and E on
  syft. They are run when a build is final, with `live.py run --held K,N,E,R,D,I,L`. Once a
  held-out session has been read, its task is a development task for every later build.

Two things were looked at on held-out tasks before the batch of version 0.2.2 ran, and neither
changed the build: whether the hook printed anything at all for a prompt (a count per family),
and what `codefast rename` does on the ten rename tasks without a model.

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

## The runs behind the tables in the README

**Held-out tasks, version 0.2.2** (`bench/results/live-heldout-haiku.jsonl`, tag `h1`): one
batch of both arms on Haiku 5.5, two runs per task and arm, interleaved so that a change in the
account's speed during the batch falls on both arms alike. A figure of the table is the mean
over the tasks of a family, each task being the mean of its runs. The sessions of four
questions (L) were run right after it with the same build, tag `h1l`.

Of the 328 runs of that batch, 144 ended in an error from the API with no tokens used, in both
arms alike. They were deleted and run again the same day, when the API answered normally. The
run of a task that had succeeded was kept. Nothing else was re-run.

One change to the build came after the batch: `rename` now tells a Go type from a method of
the same name. It was checked to leave all ten rename tasks exactly as they were, by the
compiler and in what the card says, so the table stands for the released version.

While that batch ran, some of its sessions were read, how often the prompt hook printed
anything was counted per family, and the rename command was tried on the ten rename tasks
without a model. The build was not changed.

**Development tasks, version 0.2.0** (`bench/results/live-haiku.jsonl`, one run per task and
arm, the `tag` field says which batch a run is from):

* Bug fixes (D) and kernel questions (K): one batch of both arms.
* Caller questions (N), sessions of four (L) and the control arm of the renames (R): a second
  batch of both arms, run after the receivers of Go calls were typed further. The first batch
  had these at 0.49, 0.30 and 1.11 of the control.
* The plugin arm of the renames: a later run with `codefast rename`, after the card for a
  rename request was reworded once. With the first wording the agent ran the command in 4 of 8
  sessions (0.56 of the control), with the second in 8 of 8.

The held-out tasks that gain (N, L, E and R) were run again with Sonnet 5.5, both arms, one run
per task and arm: `bench/results/live-heldout-sonnet.jsonl`. N, L and R of the development
tasks were run with Sonnet too, at version 0.2.0: `bench/results/live-sonnet.jsonl`.

`bench/results/rename-offline.jsonl` and `rename-offline-go-git.jsonl` are the checks of the
rename command without a model (`rename_offline.py`), `rename-sample*.jsonl` the random samples
judged by the compiler, and `by-name.jsonl` the count of matches by name against the type
checker.

## What a bug fix costs

The held-out bug-fix families came out at 0.99 (with the failing tests given) and 1.11 (from
the issue text alone). To see whether the 1.11 is a cost the plugin imposes or the spread of
that kind of task, the other 14 issue-only tasks, the ones kept for development, were run in
three arms (`bench/results/live-bugfix-arms.jsonl`):

| Arm | Cost | Solved |
|---|---:|---:|
| `control` | 1.00 | 7 of 14 |
| `plugin` | 0.98 | 8 of 14 |
| `lean` (the plugin without its MCP server) | 1.03 | 8 of 14 |

So fixing a bug from an issue text is about as expensive either way, and which side of 1.00 a
set of 15 tasks lands on is the spread of the family. Taking the MCP tool out does not make
the plugin cheaper there: what it saves on the tool's definition it loses in steps.

A second attempt went the other way. The shell commands of these sessions were counted: 124
greps printing 273,000 characters, 42 partial file reads printing 124,000, against 97,000 for
the test runs. The after-search note answered 34 of the 124 greps, because it asks for one
known name and these greps are alternations (`grep -n "A\|B\|C"`). So two notes were added:
one for an alternation, which answers about the first branch the index pins down, and the
outline of a file the agent reads a range of. Both fired (the note after a search went from 6
to 18 over the 14 tasks, the new one before a read 7 times), and the same tasks went from 0.98
of the control to 1.23, with the control arm within 3% of its earlier self. The agent did more
work, not less: 4.1k characters of shell output became 5.6k. The change was reverted.

That is the push-precision finding of the README met from the other side: a note that is right
but not needed is not free, and the rule stays that a hook speaks about one symbol the index
pins down and says what the agent's own output did not show.

## What a card should say

The prompt card of version 0.2.2 was settled by three small experiments on development tasks
(`bench/results/live-card-wording.jsonl`, one run per task and arm, so read them as direction).

1. **A list that can be checked with grep gets checked with grep.** The 13 caller questions on
   syft, with the card giving a method's call sites in three ways. The number of sites and the
   command that lists them: 0.31 of the control. The list itself and the command: 0.34. The
   list alone, on four of the questions: 0.64, where the same four had cost 0.37 with the
   number and the command. Handed a list, the agent checked it against a text search, which
   finds the method of that name on every type, and then read each hit to tell the types
   apart. Handed a number and a command, it ran the command and took the answer as it was.
   So the card lists call sites only for a name nothing else in the repository has, where a
   search of the agent's own finds the same places.
2. **What grep cannot find is taken as it is.** The card also gained the table a function is
   bound into and the calls that reach it through that table. Six kernel questions went from
   0.80 of the control to 0.70, and their requests from 2.7 to 2.2. Of the three that ask how
   a function is reached through a pointer, two fell to 0.43 and 0.64 and the third rose to
   1.12, which is one session each and the kind of swing a single pair shows.
3. **The code in the card does not save the read before an edit.** With the definition
   printed in the card, the agent still read the same few lines before changing them, as the
   harness's edit tool expects. Six bug-fix tasks: 4.2 requests before and 4.3 after, 0.97 and
   then 0.95 of the control. The body is worth its tokens when the prompt is a question about
   the function, and not when the prompt is a bug to fix.

## Limits

* One harness (Claude Code 2.1.323) and, unless a table says otherwise, one model (Haiku 5.5).
* Single pairs vary by a factor of two in either direction, also with two runs each. Read the
  totals.
* The kernel is code the model has seen in training, and its names are unique. A control agent
  answers such questions in three or four requests, and there is not much left to save.
* The tasks of N were chosen for names that several types share. That is where structure
  matters most, and it is not every question an agent asks.
* The messages of E are written out in the source in full. A message that is put together at
  run time (a format string with values in it) is found by its longest fixed part, which the
  kernel questions about log lines exercise and E does not.
* A bug fix is mostly the model's own work: reproducing, editing, running tests again. The
  bug-fix families show what the plugin costs or saves there, and it saves nothing.
* There is no rename family for a language without a compiler. Two attempts to build one for
  Python (sqlglot) failed for want of a reference to check against: a refactoring library
  renamed the wrong places, and a rename traced from the running tests left 12 of 16
  candidates with new failing tests. The four that passed changed three or four lines each.
