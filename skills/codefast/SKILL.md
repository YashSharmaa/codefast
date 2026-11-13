---
name: codefast
description: Answers about the code of this repository from a prebuilt index, in one call. Use it to find where a symbol is defined, read one function instead of a whole file, list who calls or uses something (also through callback tables), see what breaks when a function changes, trace how execution gets from A to B, or pick the files for a bug report or stack trace. Runs the `codefast` command.
license: MIT OR Apache-2.0
---

# codefast

`codefast` answers structural questions about the repository in the current directory. The index
builds itself on first use and follows your edits, so there is nothing to set up or refresh.

Reach for it before `grep` + reading files whenever the question is about a symbol:

| You want | Run |
|---|---|
| Anything about a symbol, a file, or free text (start here) | `codefast search vfs_read` |
| Just where it is defined | `codefast find vfs_read` |
| The body of one function / struct, or a file's outline | `codefast show vfs_read`, `codefast show fs/read_write.c` |
| Exact lines | `codefast show fs/read_write.c:534-563` |
| Every use, grouped by kind | `codefast refs vfs_read` |
| Who writes a field | `codefast refs task_struct.on_rq --only write` |
| Which function a subsystem puts in an ops table slot | `codefast refs file_operations.read_iter -p fs/ext4` |
| Who calls it (also through ops tables, callbacks, registrations) | `codefast callers ext4_file_read_iter` |
| A caller tree | `codefast callers vfs_read -d 3` |
| What it calls, and where each callee lives | `codefast callees vfs_read` |
| Before changing a function: its body, its type, the signatures of what it calls, its call sites and its tests, in one answer | `codefast context vfs_read` |
| How execution gets from A to B | `codefast path sys_read ext4_file_read_iter` |
| Which function prints a message | `codefast grep -F "Busy inodes after unmount"` |
| Where to look for a bug report or stack trace | `codefast locate "<paste the report>"` (or `codefast locate - < report.txt`) |
| The API of an installed dependency | `codefast dep` lists them, `codefast dep requests Session` looks inside one |
| After an edit: what still uses something the edit removed, renamed or gave another signature | `codefast check` |
| What the uncommitted changes added, removed and altered, by definition | `codefast changed` |

Several targets can go in one call: `codefast show foo bar baz`.

Before editing a function, start with `codefast context NAME`. It stands in for the usual
opening moves (grep for the name, read the file, look for callers, look for tests) and often
is all the reading the change needs.

To rename a function or change its parameters, `codefast search NAME` lists every place that
uses it, tests included, one line per file. After the edit `codefast check` lists the places
that still use the old name or pass a number of arguments the new signature does not take. In
a language without a compiler this is the only thing that says so before the code runs.

## Trusting the answers

The index holds every definition and every reference by name in the repository as it is on disk
right now. A `find`, `refs` or `callers` answer covers the whole repository, so there is no need
to repeat the search with grep. Names that appear only in comments, strings or unparsed files
are counted separately as text-only mentions.

A call is listed when it can be tied to its target: by scope, import, or the type of the
receiver. Where the receiver's type is written nowhere (common in Python and JavaScript) a call
can only match by method name. Such lines are marked `by name`. They are listed when few types
have a method of that name, and counted otherwise (`codefast refs NAME --loose` lists them).

## Reading the answers

- Locations are `path:line`. Open them directly with your read / edit tools.
- Source listings number the first and every fifth line.
- A search card lists every use site as `path: function:line`, code first and tests after.
  `registered / bound`, `dispatched via`, `passed as a callback` and `used as a value` say how
  a function is reached when nothing calls it by name: through a table slot, an alias, or a
  callback.
- Calls on a receiver whose type is not written are shown with their source line when there
  are only a few. Each of them may belong to another type's method of the same name: the line
  is there so that a glance settles it.
- `text-only mentions` lists files where the name appears only in comments, strings or files that
  are not parsed. Those are not references, but check them before a rename.
- When an answer ends with `… N more`, narrow it instead of switching tools: `-p DIR` restricts
  to a path, `-k fn|struct|macro|field|var` to a kind, `--budget N` raises the size limit.

## Options

- `-p PATH` restrict to paths containing PATH (or a glob such as `drivers/net/**`). Can be repeated.
- `-k KIND` restrict to a symbol kind: `fn`, `type`, `struct`, `class`, `macro`, `field`, `var`, `const`.
- `--budget N` answer size in tokens (default 1500, `0` for unlimited).
- `--tests` include test code, which searches skip by default.
- `Owner.member` names a field or method of one type: `file_operations.read_iter`.
- A kind word disambiguates: `codefast search 'struct file'`.

## When plain grep is still right

Exact strings in configuration, documentation or generated files, and patterns that are not
about code structure. `codefast grep` covers most of these too and shows the enclosing function
of every match.
