# verso: semantic versions, constraints and a dependency solver

Build a Rust library crate named `verso` with a command line binary of the same name. It parses semantic versions, matches them against constraints, reads a small package registry and picks one version per package so that every dependency requirement holds. The project will be checked by tests written against the interface below, so names, signatures and output formats must match exactly.

## Layout and public interface

`Cargo.toml` with package name `verso`, edition 2024, no dependencies. `src/lib.rs` declares the public modules `version`, `constraint`, `registry`, `solver` and `error` (files `src/<module>.rs`) and re-exports `Version`, `Constraint`, `Registry`, `Error` and the function `resolve` at the crate root. `src/main.rs` is the binary.

- `error::Error`: `#[derive(Debug, Clone, PartialEq, Eq)] pub enum Error { InvalidVersion(String), InvalidConstraint(String), Parse { line: usize, message: String }, UnknownPackage(String), NoSolution }`. It implements `std::fmt::Display` and `std::error::Error`. Display text: `invalid version: <s>`, `invalid constraint: <s>`, `line <line>: <message>`, `unknown package: <name>`, `no solution`.
- `version::Version`: `#[derive(Debug, Clone, PartialEq, Eq, Hash)] pub struct Version { pub major: u64, pub minor: u64, pub patch: u64, pub pre: Vec<String> }` with `pub fn new(major: u64, minor: u64, patch: u64) -> Version` (empty `pre`), `pub fn parse(s: &str) -> Result<Version, Error>`, `pub fn is_prerelease(&self) -> bool`, and implementations of `Ord`, `PartialOrd`, `Display` and `FromStr<Err = Error>`.
- `constraint::Constraint`: `#[derive(Debug, Clone)] pub struct Constraint` (fields private) with `pub fn parse(s: &str) -> Result<Constraint, Error>`, `pub fn matches(&self, v: &Version) -> bool`, `pub fn max_satisfying<'a>(&self, versions: &'a [Version]) -> Option<&'a Version>` (the greatest version in the slice that matches, `None` if none does) and `FromStr<Err = Error>`.
- `registry::Registry`: `#[derive(Debug, Clone, Default)] pub struct Registry` with `pub fn parse(text: &str) -> Result<Registry, Error>`, `pub fn contains(&self, name: &str) -> bool`, `pub fn packages(&self) -> Vec<String>` (names in byte order), `pub fn versions(&self, name: &str) -> Vec<Version>` (ascending, empty for an unknown name) and `pub fn dependencies(&self, name: &str, version: &Version) -> Option<&[(String, Constraint)]>` (in the order written, `None` when that package version does not exist).
- `solver::resolve(registry: &Registry, root: &[(String, Constraint)]) -> Result<std::collections::BTreeMap<String, Version>, Error>`.

## Versions

A version is `MAJOR.MINOR.PATCH` optionally followed by `-` and pre-release identifiers separated by dots. MAJOR, MINOR and PATCH are decimal numbers without leading zeros (`0` itself is fine) that fit in a u64. A pre-release identifier is a non-empty string of ASCII letters, digits and `-`. An identifier made only of digits is numeric: it must not have a leading zero (except `0` itself) and must fit in a u64. The first `-` after PATCH starts the pre-release part, so `1.0.0-x-y.z` has the identifiers `x-y` and `z`. Build metadata (`+...`) is not supported and makes the text invalid. No whitespace, prefix or suffix is allowed. Any violation gives `Error::InvalidVersion` holding the exact input text.

`Display` prints the canonical form, which for every valid input equals the input.

Ordering follows semver 2.0 precedence. Compare MAJOR, then MINOR, then PATCH numerically. If those are equal, a version without pre-release identifiers is greater than one with them. Otherwise compare the identifiers left to right: two numeric identifiers compare numerically, a numeric identifier is lower than a non-numeric one, two non-numeric identifiers compare by ASCII byte order. If all compared identifiers are equal, the version with fewer identifiers is lower. So `1.0.0-alpha < 1.0.0-alpha.1 < 1.0.0-alpha.beta < 1.0.0-beta < 1.0.0-beta.2 < 1.0.0-beta.11 < 1.0.0-rc.1 < 1.0.0`.

## Constraints

A constraint is one or more AND groups separated by `||`. A version matches the constraint when it matches at least one group. A group is one or more comparators separated by `,`, and a version matches the group when it satisfies every comparator in it and passes the pre-release rule below. Whitespace may appear at the start and end, around `,` and `||`, and between an operator and its version, nowhere else. The comparators:

- `=V`, `>V`, `>=V`, `<V`, `<=V`: compare with the full version V (three parts, pre-release allowed) using the ordering above.
- a bare full version `V` means `=V`.
- `*`: every version satisfies it.
- `^P` (caret) and `~P` (tilde), where P has one, two or three parts (`1`, `1.2`, `1.2.3`), and may carry pre-release identifiers only when it has three parts. Missing parts count as 0 for the lower bound. Each means `>=lower, <upper` with `upper` (always without pre-release) as follows.
  - `^M.m.p`: if M > 0, upper is `(M+1).0.0`. If M = 0 and m > 0, upper is `0.(m+1).0`. If M = 0 and m = 0, upper is `0.0.(p+1)`.
  - `^M.m`: if M > 0, upper is `(M+1).0.0`, else `0.(m+1).0`. So `^0.0` means `>=0.0.0, <0.1.0`.
  - `^M`: upper is `(M+1).0.0`. So `^0` means `>=0.0.0, <1.0.0`.
  - `~M.m.p` and `~M.m`: upper is `M.(m+1).0`. `~M`: upper is `(M+1).0.0`.
  - Example: `^1.2.3-beta` means `>=1.2.3-beta, <2.0.0`.

Pre-release rule: a version with pre-release identifiers matches a group only if some comparator of that same group was written with a version that has pre-release identifiers and the same MAJOR, MINOR and PATCH. For `^` and `~` this is the version written after the operator. So `>=1.0.0-alpha` matches `1.0.0-beta` and `1.1.0` but not `1.1.0-alpha`, `<2.0.0` does not match `2.0.0-alpha`, and `*` matches no pre-release version on its own. A pre-release named in one group does not help another group.

Anything else is invalid and gives `Error::InvalidConstraint` holding the exact input text: an empty constraint, an empty group or comparator (`1.0.0,`, `|| 1.0.0`), an unknown operator, a partial version with an operator other than `^` and `~`, any invalid version text.

## Registry text format

The text is read line by line, and every line is trimmed of surrounding whitespace. Blank lines and lines starting with `#` are ignored (there are no comments at the end of a line). A line starting with `[` is a section header `[name version]`: the text between `[` and the final `]` holds exactly two whitespace-separated words, a package name and a valid version. A package name is one or more ASCII letters, digits, `-` or `_`. Every other line is a dependency line `dep = constraint` of the most recent section, split at its first `=` (so `b = =1.0.0` is allowed), with both sides trimmed. A section may have no dependency lines. Dependencies may name packages that have no section.

Errors are `Error::Parse` with the 1-based line number of the offending line (the message text is free): a header without the final `]` or not holding exactly a valid name and a valid version, the same name and version in two headers, a dependency line before the first header, a non-header line without `=`, an invalid dependency name, an invalid constraint, the same dependency name twice in one section.

## Solving

A solution is a map from package names to versions such that:
1. every assigned version exists in the registry,
2. every root requirement `(name, constraint)` names an assigned package whose version matches the constraint,
3. for every assigned package, every dependency of its assigned version names an assigned package whose version matches the constraint,
4. every assigned package is reachable: it is named by a root requirement or by a dependency of an assigned version of a reachable package.

`resolve` returns the greatest solution in this order: walk all package names in byte order, and at the first name where two solutions differ the one with the higher version wins, where a package missing from a solution ranks below every version. Note that this can prefer a lower version of a later name: if root requires `z` with `z 2.0.0` depending on `b` and `z 1.0.0` depending on `a`, the answer is `a` and `z 1.0.0`, because `a` is compared first. The same package may appear in several root requirements, and then all of them must hold. An empty root gives an empty map.

Errors: if a root requirement names a package that has no section, return `Error::UnknownPackage` with the first such name in root order (this check comes first). If there is no solution, return `Error::NoSolution`. A dependency on a package that has no section can never be satisfied.

## Command line

`verso <command> <args...>`. Output goes to stdout, each line ending in `\n`.

- `verso match VERSION CONSTRAINT` prints `yes` or `no`, exit 0.
- `verso max CONSTRAINT VERSION...` prints the greatest matching version, or `none` (also when no versions are given), exit 0.
- `verso sort VERSION...` prints the versions in ascending order, one per line, keeping duplicates, exit 0.
- `verso resolve FILE NAME@CONSTRAINT...` reads the registry file, splits each argument at its first `@`, resolves, and prints one `name version` line per assigned package in byte order of names, exit 0. With no requirements it prints nothing.

On a library error print `error: ` followed by the error's Display text to stderr. The exit code is 1 for `NoSolution` and 2 for every other error. An unreadable file prints `error: cannot read FILE` and an argument without `@` prints `error: invalid requirement: ARG`, both exit 2. No command, an unknown command or a wrong number of arguments (`match` takes exactly two, `max` and `resolve` at least one) prints a line starting with `usage:` to stderr and exits 2.

## Constraints on the work

Rust 1.99 with edition 2024, standard library only, no network. Put your own tests anywhere except files named `tests/hidden_*.rs`, which are reserved for the checking tests.
