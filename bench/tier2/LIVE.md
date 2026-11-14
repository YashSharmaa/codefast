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
| `oracle` | nothing of codefast, and a hook of the task's own that states the file and function of the real fix (see "What a bug fix can save at best") |
| `oracle-context` | the same, and what `codefast context` knows about that function |

Every session runs with the project's settings only (no user plugins, no cloud connectors), in
bypass-permissions mode inside a private copy of the repository, with the web tools and
anything that outlives the session switched off. Tasks that change nothing (questions) share
one checkout and one index. The plugin arms start with an index that exists, which is the state
of a project from its second session on.

## Tasks

| Family | Task | Checked by |
|---|---|---|
| Z | The same question in TypeScript: functions and methods of zod whose name something else in the project shares | the call graph the TypeScript type checker built (`bench/oracles/zod`), scored the same way |
| N | Which functions call this method? Methods of a Go repository whose name several types share, so that a text search finds more than the answer | the call graph the Go type checker built (`bench/oracles`): F1 of the callers named with their file, against all real callers and against the callers of the other types' methods of that name |
| L | Four N questions in one session | the same, per question |
| E | A run ended with this error message: which function creates it, and which functions call that function? The message stands in one place of the source, written out in full | the function around that line and its callers in the type checker's call graph |
| K | Questions about the Linux kernel from `bench/k3` | the verified answers of `bench/k3` |
| D | Make failing tests pass: issues of Django and sqlglot from Loc-Bench, at the commit before the fix, with the tests of the fix added | the tests of the fix pass and are unchanged |
| I | The same issues without their tests: the issue text alone | the tests of the real fix, copied into the tree after the session, pass |
| R | Rename one Go method everywhere, where other types have a method of the same name | `go build` and `go vet` pass, and no other definition was renamed |
| W | Write code: one well-tested function of gitleaks has its body replaced by a panic, and the session has to write it again from what the rest of the project expects of it | the tests of that function's package pass, and no test file and no file of test data changed |

`make_n_tasks.py`, `make_e_tasks.py`, `make_d_tasks.py`, `make_i_tasks.py`, `make_r_tasks.py`
and `make_w_tasks.py` build them. None of the builders uses codefast to choose a task or to
judge an answer. A W task has to compile with the body gone and has to fail its package's
tests, and the package's tests have to pass with the body there, so the failing tests are the
whole of what the session is told.

## Development tasks and held-out tasks

Hooks are shaped by reading sessions, and a figure measured on the sessions that were read says
little. So the tasks are split.

* **Development tasks**: N, L and R on syft, 18 kernel questions and 12 bug-fix tasks. Every
  session of these may be read, and what the hooks say may be changed because of what was
  seen. The table "The tasks the hooks were developed on" in the README is from these. A few
  more (`DEV_IDS` in `live.py`) were used before any table existed and are never reported.
* **Held-out tasks** (`held_out` in `tasks.json`): tasks no session had been run on. N, L, R
  and half of E are on go-git, a repository nothing in codefast had been run on. The others are
  18 more kernel questions, the 15 bug-fix tasks not used before (as D and as I), E on syft,
  and the 8 code-writing tasks (W), which no build was looked at on at all. They are run when a
  build is final, with `live.py run --held K,N,E,R,D,I,L,W`. Once a held-out session has been
  read, its task is a development task for every later build.

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

**TypeScript, version 0.2.6** (`bench/results/live-heldout-haiku.jsonl`, tag `nz1`): the same
kind of question on zod, a repository no session had used, two runs per task and arm. A session
costs 0.53 of the control and is cheaper in 8 of 8 pairs, with the answers as good. Four of the
eight questions are about the `create` of a v3 class, a name 34 other classes carry: both arms
score about 0.4 on those and 1.00 on the rest, which is where the 0.62 in the table comes from.

**Held-out tasks, version 0.2.2** (`bench/results/live-heldout-haiku.jsonl`, tag `h1`): one
batch of both arms on Haiku 5.5, two runs per task and arm, interleaved so that a change in the
account's speed during the batch falls on both arms alike. A figure of the table is the mean
over the tasks of a family, each task being the mean of its runs. The sessions of four
questions (L) were run right after it with the same build, tag `h1l`.

The bug-fix rows of the README's table are four runs of each arm on the released build (tags
`h1`, `d4` and `prec1` for D, `h1`, `i4`, `i5` and `inc` for I), the rest two runs on 0.2.2,
whose hooks answer those shorter prompts in exactly the same way.

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

A third attempt is the one that stayed. A report of a problem names no symbol for the index to
answer about, so the card for it is the ranking of the files whose text its words match, with
the best symbol of each. That ranking is the one `locate` makes, and on these very reports it
puts the right file first for 0.41 of them and in the first three for 0.90, which is above the
break-even precision the push study reports (0.45 to 0.68 for the top candidates). It fires
only for a prompt of 900 bytes over 12 lines or more: every kind of question an agent is asked
about code is shorter than that, and a report is not.

Over two batches and 26 tasks, pooled against two or three control runs each:

| | Cost | Requests | Solved |
|---|---:|---|---|
| Fix a bug, failing tests given (12) | 0.96 | 5.0 and 4.6 | 12 and 12 of 12 |
| Fix a bug from the issue text alone (14) | 1.02 | 6.9 and 6.8 | 8.0 and 9.0 of 14 |

So the card takes a step or two out of a bug fix and does not make it cheaper. Single tasks run
from 0.58 to 1.43 of the control, which is the spread of this family and the reason both
figures are pooled over four runs per task.

## What a bug fix can save at best

Three attempts had moved the figure by a few percent in either direction, so the next question
was how much there is to win at all. The `oracle` arm answers it: the session is told, before
it starts, the file and the function the real fix changed, by a hook of its own, for nothing.
No tool that finds where a change belongs can do better than that.

| Arm | Issue text alone (14) | Failing tests given (12) |
|---|---:|---:|
| control | 1.00 | 1.00 |
| the plugin as released | 0.98 | 0.98 |
| told the file and function for free | **0.95** | **0.97** |

Averaged over two or three control runs per task, because a single run of one arm differs from
its own average by 4 to 6%. Perfect localisation, handed over at no cost, is worth less than
that noise, and the plugin already reaches it.

A second oracle says where the 5% is not the whole story. Told the gold function **and what
`codefast context` knows about it** (its body, what it calls, who calls it, the tests that
reach it), a session costs **0.86** of the control, against 0.95 for the location alone. So
there is 14% in a bug fix for a push that lands on the right function with its code.

Trying to collect it showed why it stays there. The card was given the body of the
best-ranked function, which the ranker picks right (a file of the real fix) 0.45 of the time:

| What the card carried | Cost, issue text alone | Requests |
|---|---:|---|
| the ranking only (version 0.2.3) | 1.01 | 7.0 and 6.6 |
| the ranking and the body of the best-ranked function of the first file | 1.00 | 7.0 and 6.2 |
| the ranking and the body of the best-ranked function of the repository (right 0.55) | 1.00 | 7.0 and 6.5 |
| the gold function and its body | 0.86 | 6.9 and 5.8 |

Four runs per arm against four control runs. Two runs had said 0.92 for the second row, which
is what this family's spread does to two runs. The body takes requests out and leaves the cost
alone, so it was not kept.

All of those arms ran on a build whose ranking was worse than it should have been: the path
experiment below was swept into a commit by a careless `git add -A` and went out in version
0.2.3, where it cost the card's own precision (a file of the real fix among the first three for
0.57 of the reports instead of 0.79). Version 0.2.4 takes it out. The arms above are therefore
a floor for what a card can do, not a ceiling.

Version 0.2.4, with the ranking as it should be, was run on the same 14 reports twice: the
median task costs 1.02 of the control and the batch 1.19, the second figure carried by five
sessions where the agent wandered (9 requests became 16). The median is the honest reading of
this family, and it says the same as everything else here.

Three ways of raising the precision itself were tried and measured on the development reports
and on the development halves of the localisation sets: scoring path components (the one that
reached 0.2.3 by accident), stronger length normalisation (`B` 0.4 to 0.75, which lifts the
reports from 0.29 to 0.36 at the first file and drops ContextBench from 0.684 to 0.474) and
damping a file that no code-shaped word of the query matches (no effect: every candidate
already matches one). None was kept.

## What a card for a report is worth

The arms above all pushed one guess: the ranking, or the body of the best-ranked function.
They were chasing the wrong number. The ranker puts a file of the real fix first for 0.36 of
the development reports, but among the first three for **0.79**, and the oracle says the body
is what pays. So version 0.2.5 carries the body of the best function of each of the three
files it names, as `context` tells it, 300 tokens each.

| Arm | Issue text alone (14 reports) | Failing tests given (12) |
|---|---:|---:|
| control | 1.00 | 1.00 |
| the ranking alone (0.2.3, 0.2.4) | 1.01 to 1.19 | 0.97 to 1.19 |
| the body of the best-ranked function | 1.00 | - |
| **the bodies of all three** | **0.95** | 1.09 |

Four card runs against six control runs for the first column, two against four for the second.
Requests fall from 7.2 to 6.2, the fixes are the same (the card solved 6 to 9 of the 14 over
its runs, the control 7 to 9 over its own), and the saving sits in the long sessions: the
median task is unchanged and 7 of 14 are cheaper.

The second column is why the card asks a question first. Where a failing test is given, the
agent already knows where to start, and three function bodies are tokens for nothing: 1.09.

Then the held-out tasks were run on the shipped build, and the saving did not reproduce:

| | Development tasks | Held out |
|---|---:|---:|
| issue text alone | 0.95 (14 reports, 4 runs) | **1.03** (15 reports, 2 runs) |
| failing tests given | 1.09 with bodies | **1.04** with the ranking card, 0.99 with no card |

So the card is neutral on a bug fix, and the 0.95 belongs to the fourteen reports it was built
on. One refinement did carry over, because it takes text away rather than adding it: a file
whose symbol the prompt cards already explained gets no body, nothing is said twice, and the
held-out reports went from 1.03 to **0.96** with 6.9 requests down to 6.4. The fixes in that
run were 6.5 of 15 against the control's 7.5, which is one task in a run and inside the band
this family resolves: three card arms have come in at 7.0, 6.5 and 6.5 against a control
measured once at 7.5, and nothing here separates them. What the held-out runs did settle is
one rule: where a report names a test file of this repository the card says nothing at all,
which restores every fix (15 of 15 against 14.5) and takes the median task to 0.95. That rule
is why the card never fires on the family with the failing tests given, whose every prompt
names one, and it is the reason the two families had to be read apart from here on.

## The tests that reach what an edit changed

The expensive bug-fix sessions are not expensive because the agent cannot find the code. They
are expensive because of the loop after the edit: run the tests, read the output, run them
again. In one held-out session a single test run printed 78 KB. codefast knows which tests
reach a function, so a hook was added to say so after an edit changed one:

```
the tests that reach create_permissions, which this edit changed: tests/auth_tests/
test_management.py: test_default_permissions, test_unavailable_models,
test_create_permissions_checks_contenttypes_created, test_set_permissions_fk_to_using_parameter
```

On that session's own task the line names the test that was failing. On the development
reports it did what it was meant to do, and on the held-out tasks it did not:

| | Cost | Median task | Shell output | Fixes |
|---|---:|---:|---:|---|
| 14 development reports, 4 runs | **0.93** | 0.96 | 6.5k to 4.3k | 7.8 against 8.0 of 14 |
| 15 held-out reports, 2 runs | 1.03 | 1.09 | 8.9k to 8.3k | 6.5 against 7.5 of 15 |
| 15 held-out tasks with their tests, 2 runs | 1.02 | **0.88** | 11.6k to 9.6k | 15 of 15 |

It was reverted. The one reading worth keeping is the last row: where the tests of the change
are in the tree, the median session falls to 0.88 and every fix is kept, and the total is
carried by a few long sessions that the line does not shorten. Where the benchmark removes
those tests (the issue-only family), the line names tests that cannot check the fix, and it
reads as the push-precision rule again: a fact that is true and not the one needed costs.

What that leaves is a number to beat rather than a wall: the push is worth 14% at a selection
precision of 1.00 and nothing at 0.45, which is the break-even band the push study reports
(0.45 to 0.68). Raising what `locate` does with the text of a report above that band is the
work that would make a bug fix cheaper.

The reason the rest of the session does not move is in what the cost is made of. Output tokens weigh five times an input token, and
they are 28 to 60% of a session in every family. Where codefast saves, it saves there: a caller
question goes from 8,477 output tokens to 2,731, four of them in one session from 42,942 to
7,860, a rename from 5,100 to 2,823. A bug fix writes the fix and runs the tests, and that
output does not shrink: 4,536 to 4,335 with the failing tests given, 4,510 to 4,805 without
them. An index shortens the finding, and finding is not what a bug fix spends.

## Taking the card out again

Five attempts had now been measured on the held-out bug-fix tasks, and not one of them paid.
The question left was the plain one: what does the family do with no card for a report at all?
The `no-report` arm answers it, and version 0.2.9 is that arm shipped.

Every row below is two runs of 15 tasks of one build, scored against the same four control runs
of the same tasks, so the rows are comparable to each other and to the README's table.

Fix a bug from the issue text alone (15 reports, the control at 7.1 requests and 7.5 fixes):

| Build | What the prompt hook added for a report | Cost | Median task | Requests | Fixed |
|---|---|---:|---:|---:|---:|
| 0.2.5 (`h25`) | the ranking and the bodies of its three files | 1.01 | 0.96 | 7.1 | 7.0 |
| 0.2.6 (`h27`) | the same, and the tests that reach the edit | 1.02 | 1.06 | 6.7 | 6.5 |
| 0.2.6 (`h28`) | the same, that note reverted | 0.95 | 0.96 | 6.4 | 6.5 |
| 0.2.8 (`i5`) | **nothing** | 0.96 | 1.02 | 6.8 | 7.5 |
| 0.2.8 (`inc`) | **nothing** | 0.89 | 0.93 | 6.2 | 7.0 |

Fix a bug, failing tests given (15 issues, the control at 6.3 requests and 15 fixes). Every
prompt of this family names a test file of the repository, so from 0.2.6 on the card says
nothing here by the rule above, and the only build that ever carded these prompts is 0.2.5:

| Build | Cost | Median task | Requests | Fixed |
|---|---:|---:|---:|---:|
| 0.2.5 (`h25`), the ranking card | 1.01 | 1.03 | 6.0 | 14.5 |
| 0.2.5 (`h26`), no card on these prompts | 1.01 | 0.98 | 6.1 | 15 |
| 0.2.6 (`h27`), and the tests that reach the edit | 1.00 | 0.93 | 5.8 | 15 |
| 0.2.7 (`prec1`) | 1.00 | 0.98 | 6.3 | 15 |
| 0.2.8 (`d4`) | 0.87 | 0.94 | 5.3 | 15 |

Pooled over four runs each, the issue-only family costs **0.98** with a card (`h25` and `h28`,
6.8 fixes) and **0.92** without one (`i5` and `inc`, 7.2 fixes). Two runs of one build differ
from two other runs of the same build by as much as the two columns differ from each other
(0.96 and 0.89 are both 0.2.8), so the reading is not that 0.92 beats 0.98. It is that over
five builds and ten batches the card never once paid for its tokens, while the arm with no card
is the only one that also keeps every fix. So it was removed, with the constants, the test and
the `CODEFAST_REPORT_CARD` switch, and what the prompt hook says about a report is now nothing.

The README's two bug-fix rows are the four runs of the released behaviour: `prec1` and `d4` for
the tests-given family at 0.93, `i5` and `inc` for the issue-only family at 0.92, each against
the four control runs. Their median tasks are 0.99 and 1.03, so what the totals save sits in a
few long sessions and not in the typical one.

### Why the line stops here, and it is the input and not the ranking

Six arms had moved nothing, so the last question was asked of the text rather than of the
ranker: what does a bug report name that an index can state as a fact? Measured on the 14
development reports, with the repository indexed (`exact.py` and `sel.py` of this directory):

| What the report names | Reports |
|---|---:|
| a file path that exists in the tree | **1** of 14 |
| that path is a file the real fix changed | 1 of 14 |
| an identifier the index resolves to exactly one definition | 14 of 14 |
| one of those definitions is in a file the real fix changed | 11 of 14 |
| one of them **is** a function the real fix changed | 4 of 14 |

So a report does name symbols, and the index does resolve them, but which of the ten or twelve
it names is the one that matters is not in the text. Eight ways of choosing one were scored:

| Which named symbol the card would carry | Fires | Its file is a gold file | It is a gold function |
|---|---:|---:|---:|
| the first in the title | 12 | **0.58** | 0.17 |
| the first in the report | 14 | 0.50 | 0.14 |
| the most mentioned | 14 | 0.50 | 0.14 |
| the first that is called, as `f(` | 11 | 0.18 | 0.00 |
| the first after a dot, as `x.f` | 10 | 0.10 | 0.00 |
| the first in backticks or quotes | 3 | 0.00 | 0.00 |

The best of them, 0.58 at the file, sits inside the break-even band of the push study (0.45 to
0.68), and an arm already measured at 0.55 bought nothing. At the function, which is where the
oracle's 14% lives, the best selector is right 0.17 of the time. One of its picks is the word
`recursively`, which happens to be a symbol name in Django, so even the exactness is thin.

That is the finding: for an issue text of this kind the information a push would need is not in
the prompt, and no ranking of the index can put it there. It is not a tuning failure. The
families where codefast saves 0.15 to 0.63 are the ones whose prompt names what it is about,
and this one does not.

## One meaning or nothing

The note after a search picked the first of several things a name stands for when none of them
stood out. In a held-out session an agent grepping Django's test client for `context` was told
about `DecimalField.context`, and that session ran to 19 requests where the control took 10.
The note now says nothing unless the name means one thing in this repository.

| | Cost | Median task | Right |
|---|---:|---:|---:|
| 15 held-out tasks with their tests, before | 1.04 | 0.95 | all fixed |
| the same, after | 1.02 | 0.97 | all fixed |
| 12 held-out caller questions, before | 0.28 | 0.28 | 0.98 |
| the same, after | 0.30 | 0.34 | 0.99 |

Two runs per arm, so the cost columns say the same thing twice: nothing moved outside the
noise. What moved is what the tool says, and the answers got slightly better rather than
worse, which is the trade the project's own rule asks for.

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
  bug-fix families show what the plugin costs or saves there, and the answer is that it costs
  nothing and saves little. Their medians are level with the control and the totals, 0.93 and
  0.92, come from a few long sessions. Six attempts to make that figure move are above.
* There is no rename family for a language without a compiler. Two attempts to build one for
  Python (sqlglot) failed for want of a reference to check against: a refactoring library
  renamed the wrong places, and a rename traced from the running tests left 12 of 16
  candidates with new failing tests. The four that passed changed three or four lines each.
