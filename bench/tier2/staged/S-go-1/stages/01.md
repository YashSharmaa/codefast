# buildplan: a build-dependency planner

buildplan reads a small Makefile-like build file, orders its targets so that dependencies come first, reports dependency cycles, and decides which targets must be rebuilt given the modification times of files. It is a Go library with a command line. The project will be checked by tests written against the interface below, so names, signatures, messages and output formats must match exactly.

## Module and packages

Module path `example.com/buildplan`, `go 1.22` in go.mod, standard library only. Packages: `parse`, `graph`, `plan`, `cli`. A thin `cmd/buildplan/main.go` may call `cli.Run(os.Args[1:], os.Stdout, os.Stderr)` and exit with its result.

## Package parse

```go
type Target struct { Name string; Deps []string; Commands []string; Line int }
type File struct { Vars map[string]string; Targets []*Target } // Targets in declaration order
func (f *File) Target(name string) *Target // nil when not declared
type ParseError struct { Line int; Msg string }
func (e *ParseError) Error() string        // "line N: MSG", for example "line 3: bad line"
func Parse(src string) (*File, error)      // errors are always *ParseError
```

The text is split on `\n`, and one trailing `\r` is removed from each line. Lines are numbered from 1. Each line is one of these, checked in this order:

1. Blank (only spaces and tabs) or a comment (the first non-blank character is `#`): ignored.
2. Indented (starts with a space or tab): a command of the most recent target. Its text is the line with surrounding whitespace removed, then expanded. If no target has been declared yet, the error is `command before any target`. Blank and comment lines between commands do not end a target's command list.
3. Assignment: the line starts with a name (`[A-Za-z_][A-Za-z0-9_]*`), then optional spaces or tabs, then `=`. The value is everything after the first `=` with surrounding whitespace removed, expanded at once. A later assignment replaces the value for the lines after it. `File.Vars` holds the final values.
4. Target line: any other line that contains `:`. The name is the text before the first `:`, trimmed, then expanded. If the expanded name is empty or contains a space or tab, the error is `bad line`. If a target with that name is already declared, the error is `duplicate target "NAME"` (Go `%q` quoting) on the later line. The text after the first `:` is expanded and then split on whitespace into dependencies. A name repeated in one dependency list is kept only at its first position. `Line` is the line number of the target line.
5. Anything else: `bad line`.

Expansion replaces each `$(NAME)`, where NAME matches the name pattern above, with the current value of that variable, scanning left to right in one pass (the inserted text is not scanned again). A `$` that does not start such a reference is kept as is, for example `$$`, `$x` or `$(1)`. A reference to a variable that has not been assigned on an earlier line gives `undefined variable "NAME"`. On a target line the name is expanded and checked for duplicates before the dependency text is expanded. Parsing stops at the first error.

## Package graph

```go
type Graph struct { /* unexported */ }
func New() *Graph
func (g *Graph) AddNode(name string)            // no-op when present
func (g *Graph) AddEdge(from, to string)        // "from depends on to"
func (g *Graph) Nodes() []string                // insertion order
func (g *Graph) Deps(name string) []string      // insertion order, empty for unknown names
func (g *Graph) Order(goals []string) ([]string, error)
func (g *Graph) Cycles() [][]string
func FromFile(f *parse.File) *Graph
type CycleError struct { Path []string }
func (e *CycleError) Error() string             // "cycle: " + Path joined by " -> "
```

`AddEdge` adds `from` and then `to` as nodes if they are missing, and ignores an edge that already exists. `FromFile` goes through the targets in declaration order and for each one calls `AddNode(target)` and then `AddEdge(target, dep)` for each dependency in order. So dependencies that are not targets (plain files) are nodes too.

`Order` is a depth-first search. It visits the goals in the given order. Visiting a node that is already finished does nothing. Otherwise the node goes on the current search path, its dependencies are visited in their insertion order, and then the node leaves the path and is appended to the result. A goal that is not a node is treated as a node without dependencies. If the search reaches a node that is on the current path, `Order` returns nil and a `*CycleError` whose `Path` is the part of the current path from that node to the end, followed by that node again. Example: with `a: b`, `b: c`, `c: b`, `Order([]string{"a"})` fails with Path `[b c b]`, and a self-dependency `x: x` gives `[x x]`. Without a cycle the result holds every node reached, each once, dependencies before dependents.

`Cycles` returns every strongly connected component of the whole graph that has more than one node, or has one node with an edge to itself. Each component is sorted ascending and the list is sorted by first element. With no cycle the result is empty (length 0).

## Package plan

```go
type State interface { ModTime(name string) (int64, bool) } // false when the file does not exist
type MapState map[string]int64                               // implements State
type Step struct { Target string; Reason string; Commands []string }
type MissingError struct { Name, NeededBy string }
func (e *MissingError) Error() string // `missing "NAME" needed by "BY"`, or `missing "NAME"` when NeededBy is ""
var ErrNoTargets = errors.New("no targets")
func DefaultGoals(f *parse.File, goals []string) ([]string, error)
func Make(f *parse.File, goals []string, st State) ([]Step, error)
```

`DefaultGoals` returns `goals` unchanged when it is not empty, else a slice holding the first declared target, else `ErrNoTargets`.

`Make` applies `DefaultGoals`, then computes `graph.FromFile(f).Order(goals)` and returns any `*graph.CycleError` unchanged. It then walks the order. A name that is not a declared target must exist in `st`, otherwise `Make` returns a `*MissingError` for the first such name in the order, where `NeededBy` is the declared target that comes first in the order among those whose dependency list contains the name ("" when there is none, for example a missing goal that nothing depends on). A declared target runs when the first of these rules applies, checked in this order:

1. `missing`: the target itself does not exist in `st`.
2. `dependency D is being rebuilt`: D is the first dependency, in declared order, that runs earlier in this plan.
3. `dependency D is newer`: D is the first dependency, in declared order, that exists with a modification time strictly greater than the target's.

A target to which no rule applies is up to date and gives no step. The result lists one `Step` per target that runs, in the order, with the rule's text as `Reason` (D written plainly, without quotes) and a copy of the target's commands. A target without commands still gives a step. When nothing runs the result is empty.

## Package cli

```go
func Run(args []string, stdout, stderr io.Writer) int // args exclude the program name
```

- `order FILE [GOAL...]`: prints the result of `Order` for the goals passed through `DefaultGoals` (so a file without targets and no GOAL gives the error `no targets`), one name per line. Exit 0.
- `plan FILE MTIMES [GOAL...]`: runs `plan.Make` with the MTIMES file as a `MapState`. For each step it prints `TARGET: REASON`, then each command on its own line indented by two spaces. With no step it prints `nothing to do`. Exit 0.
- `cycles FILE`: prints each component from `Cycles` with its names joined by one space, one per line, and exits 1. With no cycle it prints `no cycles` and exits 0.

The MTIMES file has one `NAME TIME` pair per line (after removing surrounding whitespace, exactly two whitespace-separated fields, TIME a base-10 int64). Lines are numbered from 1, counting every line. Blank lines and lines starting with `#` after leading whitespace are ignored. A later line for the same name wins. Any other line is the error `mtimes line N: bad line`.

Errors are written to stderr as `error: ` followed by the error text and a newline, with nothing on stdout. A `*graph.CycleError` or `*plan.MissingError` exits 1. A parse error (for example `error: line 4: bad line`), a bad MTIMES line, `no targets`, or a file that cannot be read exits 2. No arguments, an unknown command, or a wrong number of arguments (`order` needs at least FILE, `plan` at least FILE and MTIMES, `cycles` exactly FILE) prints a line starting with `usage:` to stderr, nothing to stdout, and exits 2.

## Constraints

Go 1.22 or later, standard library only, no network. Your own tests may live anywhere in the module except files named `hidden_*_test.go`, which are reserved for the checking tests.
