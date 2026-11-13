# Agent pilot: protocol

A small paired experiment. The same agent runs the same task in two or three arms, and the
arms are compared task by task. It is a pilot: one run per task and arm, tens of tasks, the
sub-agents of one coding harness. The numbers are indicative, not a full study.

## Tasks

The task ids are in `tasks.json`.

| Set | Tasks | Source | How an answer is checked |
|---|---|---|---|
| Kernel navigation | 18 + 18 | `bench/k3/questions.verified.jsonl`, two questions per family in each round | the K3 scorer, applied to the text of the answer |
| Bug localisation, Python | 6 + 6 | Loc-Bench, from the 60 task slice | all gold files among the 10 files the agent returns |
| Bug localisation, kernel | 12 | LinuxFLBench | the gold file among the 10 files the agent returns |
| Code writing | 8 | gitleaks: one well tested function has its body removed | the package's own tests pass and no test file changed |

Round 1 was used to improve the tool and its guide. Round 2 uses tasks the tool had not been
tried on.

## Arms

* **control**: shell tools only.
* **tool**: the same, plus the codefast command for that repository and its guide
  (`skills/codefast/SKILL.md`) in the prompt.
* **rule**: the tool arm plus one paragraph of project instructions that says to try codefast
  first. This is the paragraph that `codefast setup agents-md` writes.

The order of the arms rotates from task to task, so that a warm prompt cache does not favour
one arm.

## Prompts

Every arm gets the same task text. The navigation prompt:

```
You are helping a developer who is working in a source tree: <what>. The tree is at <path>.

Investigate with shell commands through the Bash tool (rg, grep, find, sed, cat and so on) and
with the Read tool. Rules: work only inside <path>. Do not read, list or run anything outside
it. Do not use the web. Do not start sub-agents or workflows. Do not write or modify files.
Stop as soon as you are confident of the answer.

QUESTION: <question>

When you are done, return the structured result. "answer" is what you would tell the
developer: name every function or definition involved together with its location in the form
path/to/file.c:123. Include only what you verified.
```

The localisation prompt replaces the question with the issue text and asks for up to 10 file
paths, most likely first. The writing prompt names the function whose body was lost, the test
command, and forbids editing tests.

The tool arm adds:

```
A code-intelligence command is installed for this tree: <path to codefast>. Wherever its
guide below says `codefast`, run that command instead. Use it when it fits the question and
use ordinary shell search when it does not.
```

followed by the guide.

## What is measured

Read from each run's transcript by `score_ab.py`:

* tokens processed: the sum over requests of the prompt size (uncached input, cache reads and
  cache writes), and the output tokens
* a cost-weighted total, with cache reads at 0.1, cache writes at 1.25 and output at 5 times
  the price of an input token
* requests, tool calls, wall time, and how often the codefast command was run
* violations: web use, sub-agents, or the control arm touching the tool

## Limits

* The agents are sub-agents of one harness. Each request carries about 40,000 tokens of
  fixed prompt, so the token totals mostly count requests. A harness with a smaller fixed
  prompt would show a larger relative difference in tool output and a smaller one in total.
* No hooks were active. The tool was reached only through its guide.
* The control agent was already frugal. It used narrow greps and line ranges rather than
  whole file reads.
* One run per task and arm. Differences of a task or two are noise.
* One of the writing tasks hit a test that fails in every copy of the project (a symlink that
  the copy turned into a file). That test is skipped when the result is checked.
