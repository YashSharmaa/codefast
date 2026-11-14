# Projects that are built and then changed, in one conversation

Six tasks for family S of `bench/tier2/live.py`. A session is asked for a project (the first
message is the specification of one of the tasks in `bench/tier2/new/`), and then, message
after message in the same conversation, for five changes to it. What this family measures is
what a development conversation costs as the project and the conversation grow: every request
sends the whole conversation again.

Each task directory holds:

- `stages/01.md` to `stages/06.md`: the six messages. The first is a specification. Each of the
  others is a request a user would type next: a feature that needs several modules to change, a
  new module wired into the old ones, a change of behaviour that was specified before, a change
  across the whole project (a rename, or a new parameter on a function called from many
  places), and a bug report that pins down something the specification left open.
- `hidden/`: the tests of the final state. No session sees them.
- `checkpoints/01` to `checkpoints/05`: the tests that hold after each earlier message, so
  that a conversation can be graded part way.
- `reference/`: an implementation of the final state that passes every hidden test.
- `task.json`: the language, the check command, the number of hidden tests, and whether the
  task is one to develop on (`dev`) or held out (`held`).

| Task | Language | Split | Hidden tests | The five changes |
|---|---|---|---:|---|
| S-go-1 | Go | dev | 96 | Pattern rules, automatic variables and phony targets. Parallel waves. Targets and variables on several lines. Pattern rules for goals. Bug: a `#` later in a line is not a comment |
| S-go-2 | Go | held | 97 | Fixed windows. Replaying traffic. Checking a batch of requests. Request costs. Bug: a query string gets around the policies |
| S-py-1 | Python | dev | 97 | Schedule files and exclusion rules. Rules in plain English. Short months and whole years. One error type that says where. Bug: spaces inside a rule are rejected |
| S-py-2 | Python | held | 98 | Joins. Scalar functions. NULL logic as in SQL. ParseError, and where an error is. Bug: quoted CSV fields lose their text |
| S-rs-2 | Rust | held | 96 | Fuzz when applying. Three-way merge. GNU style headers and patches for whole trees. Ignoring whitespace. Bug: lines after a "No newline" marker |
| S-ts-2 | TypeScript | held | 85 | Computing values: arithmetic, lists, objects and `set`. Macros, calls, imports and globals. Loops over more kinds of values, and loop control. Errors say where they happened. Bug: emoji count as two characters |

How a task was admitted: the reference was built message by message, and after each one the
tests of that checkpoint pass. The tests of a message fail on the state before it. Two small
faults per message, put into the final reference one at a time, each fail a hidden test. And
every detail a test relies on is stated in some message.

A run keeps the project as it stands after every reply (`stage-01` to `stage-06` in the run
directory), and `live.py verify` grades each against its checkpoint.

```sh
python3 bench/tier2/live.py check-new --ids S-go-1,S-py-1     # the references, graded as a session is
python3 bench/tier2/live.py run --tag s1 --held Sdev --arms control,plugin --reps 2 --jobs 2
python3 bench/tier2/live.py score --tag s1
```
