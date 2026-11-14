# Projects to create from nothing

Ten tasks for family G of `bench/tier2/live.py`: a session is given an empty directory and
`SPEC.md`, builds the project, and is graded afterwards by tests it never saw.

Each task directory holds:

- `SPEC.md`: the only thing a session is given. It states the public interface exactly, so
  that two independent implementations pass the same tests.
- `hidden/`: the tests, at the paths they have inside the project. They use nothing but the
  interface the specification gives.
- `reference/`: an implementation that passes every hidden test. It shows that the tests can
  be passed, and no session sees it.
- `task.json`: the language, the command that runs the hidden tests, their number, and whether
  the task is one to develop on (`dev`) or held out (`held`).

| Task | Language | What is built | Split | Hidden tests |
|---|---|---|---|---:|
| G-py-1 | Python | A recurring-date rule expander with schedules and a command line | dev | 38 |
| G-py-2 | Python | A SQL SELECT engine over CSV tables | held | 35 |
| G-py-3 | Python | Unit conversion with dimensional analysis | held | 33 |
| G-go-1 | Go | A build-dependency planner with cycle reporting | dev | 39 |
| G-go-2 | Go | A rate-limit policy engine with an HTTP surface | held | 39 |
| G-go-3 | Go | A segmented event journal with crash recovery | held | 39 |
| G-rs-1 | Rust | Semantic versions, constraints and a dependency solver | dev | 38 |
| G-rs-2 | Rust | A line diff with unified patches and fuzzy application | held | 35 |
| G-ts-1 | TypeScript | A meeting-room scheduler with a request handler | dev | 28 |
| G-ts-2 | TypeScript | A text template engine | held | 30 |

Every project uses the standard library of its language alone, so a session installs nothing
and needs no network. The TypeScript projects run in Node without a build step.

How a task was admitted: the reference passes every hidden test, each of 8 to 14 small faults
put into the reference one at a time fails at least one test (109 faults over the ten tasks,
all caught), and every detail a test relies on is stated in the specification.

`python3 bench/tier2/live.py check-new` grades every reference the way a session's work is
graded, and an empty project next to it: the first must pass every test and the second none.

```sh
python3 bench/tier2/live.py run --tag g1 --held Gdev --arms control,plugin --reps 2 --jobs 2
python3 bench/tier2/live.py score --tag g1
```
