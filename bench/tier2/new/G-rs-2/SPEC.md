# patchwork: line diffs and unified patches

Build a Rust library crate named `patchwork` with a command line binary of the same name. It computes minimal line diffs between two texts, groups them into hunks, writes and reads unified diff text, applies patches (tolerating moved hunks) and reverses them. The project will be checked by tests written against the interface below, so names, signatures and output formats must match exactly.

## Layout and public interface

`Cargo.toml` with package name `patchwork`, edition 2024, no dependencies. `src/lib.rs` declares the public modules `lines`, `diff`, `hunk`, `patch`, `apply` and `error` (files `src/<module>.rs`) and re-exports every item listed below at the crate root. `src/main.rs` is the binary.

- `lines::split_lines(text: &str) -> Vec<String>` and `lines::join_lines(lines: &[String]) -> String`.
- `diff::Edit`: `#[derive(Debug, Clone, PartialEq, Eq)] pub enum Edit { Equal(String), Delete(String), Insert(String) }` and `diff::diff_lines(old: &[String], new: &[String]) -> Vec<Edit>`.
- `hunk::HunkLine`: `#[derive(Debug, Clone, PartialEq, Eq)] pub enum HunkLine { Context(String), Removed(String), Added(String) }`.
- `hunk::Hunk`: `#[derive(Debug, Clone, PartialEq, Eq)] pub struct Hunk { pub old_start: usize, pub old_count: usize, pub new_start: usize, pub new_count: usize, pub lines: Vec<HunkLine> }`, and `hunk::make_hunks(edits: &[Edit], context: usize) -> Vec<Hunk>`.
- `patch::Patch`: `#[derive(Debug, Clone, PartialEq, Eq)] pub struct Patch { pub old_name: String, pub new_name: String, pub hunks: Vec<Hunk> }`, with `patch::make_patch(old_name: &str, old_text: &str, new_name: &str, new_text: &str, context: usize) -> Patch`, `patch::format_patch(patch: &Patch) -> String` and `patch::parse_patch(text: &str) -> Result<Patch, Error>`.
- `apply::Applied`: `#[derive(Debug, Clone, PartialEq, Eq)] pub struct Applied { pub text: String, pub offsets: Vec<isize> }`, with `apply::apply_patch(text: &str, patch: &Patch) -> Result<Applied, Error>` and `apply::reverse_patch(patch: &Patch) -> Patch`.
- `error::Error`: `#[derive(Debug, Clone, PartialEq, Eq)] pub enum Error { Parse { line: usize, message: String }, HunkFailed(usize) }`, implementing `Display` (`line <line>: <message>` and `hunk <n> failed`) and `std::error::Error`.

## Lines

Only `\n` separates lines. Every line keeps its terminating `\n`. The last line has none when the text does not end with `\n`. So `"a\nb"` gives `["a\n", "b"]`, `"a\n\n"` gives `["a\n", "\n"]` and `""` gives no lines. The strings `"b"` and `"b\n"` are different lines everywhere in this library. `join_lines` concatenates, so `join_lines(&split_lines(t)) == t`. The lines inside `Edit` and `HunkLine` values follow the same convention.

## Diff

`diff_lines` returns an edit script: the `Equal` and `Delete` lines in order are exactly `old`, and the `Equal` and `Insert` lines in order are exactly `new`. The script must be minimal: the number of `Delete` plus `Insert` edits equals `old.len() + new.len() - 2 * L`, where L is the length of a longest common subsequence. Inside every maximal run of consecutive non-`Equal` edits, all `Delete` edits come before all `Insert` edits. When lines repeat, several scripts may meet these rules and any of them is accepted. When every line within each text is distinct and the lines the texts share appear in the same relative order in both, the script is unique.

## Hunks

`make_hunks` turns a script into hunks with `context` unchanged lines around changes. Changes whose separating gap holds at most `2 * context` `Equal` edits belong to the same hunk, with the whole gap as context. A hunk starts `context` `Equal` edits before its first change and ends `context` `Equal` edits after its last change, cut short at the start or end of the script. Each `Equal` becomes `Context`, `Delete` becomes `Removed`, `Insert` becomes `Added`. `old_count` counts the `Context` and `Removed` lines, `new_count` the `Context` and `Added` lines. `old_start` is the 1-based number of the hunk's first old line, except when `old_count` is 0, when it is the number of old lines that come before the hunk. `new_start` follows the same rule on the new side. So inserting two lines into an empty text gives `old_start 0, old_count 0, new_start 1, new_count 2`. A script without changes gives no hunks.

`make_patch` splits both texts, diffs them and makes hunks, keeping the two names.

## Unified diff text

`format_patch` writes `--- <old_name>\n`, then `+++ <new_name>\n`, then for each hunk the header `@@ -<old_start>,<old_count> +<new_start>,<new_count> @@\n` (both numbers always present) followed by one output line per hunk line: the prefix ` ` (context), `-` (removed) or `+` (added), then the line without its `\n`, then `\n`. When a hunk line has no terminating `\n`, the next output line is `\ No newline at end of file\n`. A patch without hunks is just the two name lines.

`parse_patch` reads that format. The text is split at `\n`, and a final `\n` does not start another line. Line 1 must start with `--- ` and line 2 with `+++ `, and the names are the rest of those lines. Then come zero or more hunks, each a header line followed by its body, until the text ends. A header is `@@ -S,C +S,C @@` where all four are decimal numbers, optionally followed by a space and any text, which is ignored. A start of 0 is allowed only with a count of 0. The body is read until it holds exactly old count lines with prefix ` ` or `-` and new count lines with prefix ` ` or `+`. Body lines become hunk lines whose text is the rest of the line plus `\n`. A line starting with `\` (any text after it) may follow any body line, including the last one, and removes the `\n` from the hunk line just before it. Header numbers are kept as written.

Errors are `Error::Parse` with the 1-based line number of the first line that cannot be accepted, or the number of lines plus one when the text ends inside a hunk body. The message text is free. The cases:

- line 1 or line 2 is missing or does not start with its prefix,
- a line where a header is expected is not a valid header (this includes a body line that comes after the counts are used up),
- a body line with any prefix other than ` `, `-`, `+` or `\` (an empty line included),
- a body line whose prefix needs a count that is already used up (for example a third `-` line when the old count is 2),
- a `\` line right after a header or right after another `\` line.

For example, this text parses into two hunks. The second ends with the hunk line `"eleven"` without `\n`, and its header text `section` is ignored:

```
--- old.txt
+++ new.txt
@@ -1,3 +1,3 @@
 one
-two
+TWO
 three
@@ -10,2 +10,3 @@ section
 ten
+ten and a half
 eleven
\ No newline at end of file
```

## Applying

`apply_patch` splits the text into lines and places the hunks in order, working on the original lines. For each hunk:

- Its old lines are its `Context` and `Removed` lines in order (say `len` of them), and its new lines are its `Context` and `Added` lines in order.
- Its nominal position is the 0-based index `old_start - 1`, or `old_start` when `old_count` is 0.
- Its expected position is the nominal position plus the offset of the previous hunk (0 for the first hunk).
- It may be placed at an index k when the text lines `k .. k + len` equal its old lines exactly (newlines included), k is at or after the end of the lines the previous hunk replaced (0 for the first hunk), and `k + len` is at most the number of text lines. An empty old side fits at every such k.
- Of all allowed k, the one closest to the expected position is used, and of two equally close the lower one. The hunk's offset is k minus its nominal position.
- If no k is allowed, return `Error::HunkFailed(i)` with the hunk's 1-based number.

The result text is the original text with each placed hunk's old lines replaced by its new lines, and `offsets` holds one offset per hunk in order. A patch made by `make_patch(_, a, _, b, n)` applied to `a` gives `b` with every offset 0.

`reverse_patch` swaps `old_name` with `new_name`, `old_start` with `new_start` and `old_count` with `new_count`, turns `Removed` into `Added` and back, and then inside every maximal run of non-`Context` lines moves the `Removed` lines before the `Added` lines, keeping the order within each kind.

## Command line

`patchwork <command> <args...>`, output to stdout.

- `patchwork diff OLD NEW [-U N]` reads both files and diffs them with N context lines (default 3, N a non-negative decimal number), using the paths as given as the names. If the texts are equal it prints nothing and exits 0. Otherwise it prints `format_patch` output and exits 1.
- `patchwork apply FILE PATCH` applies the patch file to the text of FILE and prints the resulting text exactly (no extra newline). For each hunk with a non-zero offset, in order, it prints `hunk <i> offset <k>` to stderr (for example `hunk 2 offset -3`). Exit 0.
- `patchwork reverse PATCH` prints the `format_patch` output of the reversed patch, exit 0.
- `patchwork stat PATCH` prints `<old_name> -> <new_name>: <h> hunks, <a> added, <r> removed`, counting hunks and `Added` and `Removed` lines, exit 0.

Errors go to stderr as one line starting with `error: `. A library error prints `error: ` and its Display text, with exit 1 for `HunkFailed` and 2 for `Parse`. An unreadable file prints `error: cannot read <path>` and an invalid N prints `error: invalid context: <N>`, both exit 2. A missing or unknown command or wrong arguments print a line starting with `usage:` to stderr and exit 2.

## Constraints on the work

Rust 1.99 with edition 2024, standard library only, no network. Put your own tests anywhere except files named `tests/hidden_*.rs`, which are reserved for the checking tests.
