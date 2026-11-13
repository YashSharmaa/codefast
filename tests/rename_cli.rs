//! `codefast rename`: a small project is written to a temporary directory and renamed through
//! the command line, the way an agent would. What is checked is the files afterwards.

use std::path::PathBuf;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_codefast");

struct Project {
    root: PathBuf,
}

impl Project {
    fn new(name: &str, files: &[(&str, &str)]) -> Project {
        let root = std::env::temp_dir().join(format!("codefast-rename-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (rel, text) in files {
            let p = root.join("src").join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, text).unwrap();
        }
        Project { root }
    }

    fn ask(&self, args: &[&str]) -> String {
        let out = Command::new(BIN)
            .arg("--root")
            .arg(self.root.join("src"))
            .args(args)
            .env("CODEFAST_INDEX_DIR", self.root.join("index"))
            .output()
            .expect("run codefast");
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.root.join("src").join(rel)).unwrap()
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn has(text: &str, needle: &str) {
    assert!(text.contains(needle), "expected `{needle}` in:\n{text}");
}

fn lacks(text: &str, needle: &str) {
    assert!(!text.contains(needle), "did not expect `{needle}` in:\n{text}");
}

const SETS: &str = r#"package sets

// Set holds numbers. Add puts one in.
type Set struct{ items []int }

func (s *Set) Add(x int) { s.items = append(s.items, x) }

// Bag is another type with an Add of its own.
type Bag struct{ n int }

func (b *Bag) Add(x int) { b.n += x }

func NewSet(xs ...int) *Set {
	s := &Set{}
	for _, x := range xs {
		s.Add(x) // Add each one
	}
	return s
}

func open() (string, *Set, error) { return "", nil, nil }
"#;

const USE: &str = r#"package sets

import "fmt"

func fill(b *Bag) {
	s := NewSet(1)
	s.Add(2)
	b.Add(3)
	_, t, _ := open()
	t.Add(4)
	fmt.Println("call s.Add(5) to Add more")
	both(s, b)
}

func both(s *Set, b *Bag) { s.Add(6); b.Add(7) }

func untyped(xs []interface{ Add(int) }) {
	for _, x := range xs {
		x.Add(8)
	}
}
"#;

const USE_TEST: &str = r#"package sets

import "testing"

func TestAdd(t *testing.T) {
	s := NewSet()
	s.Add(1)
}
"#;

fn sets(name: &str) -> Project {
    Project::new(name, &[("sets/sets.go", SETS), ("sets/use.go", USE), ("sets/use_test.go", USE_TEST)])
}

#[test]
fn a_method_is_renamed_where_it_is_defined_and_where_it_is_surely_called() {
    let p = sets("go");
    let said = p.ask(&["rename", "Set.Add", "Insert"]);
    has(&said, "renamed Set.Add to Insert: its definition and 4 references in 3 files");
    has(&said, "sets/sets.go: 6,16");
    has(&said, "sets/use.go: 7,10");
    has(&said, "sets/use_test.go: 7");
    // The definition and the calls on a `*Set`, also the one typed by the second result of a
    // call, and the one in a test.
    let sets = p.read("sets/sets.go");
    has(&sets, "func (s *Set) Insert(x int) {");
    has(&sets, "\t\ts.Insert(x) // Add each one");
    let uses = p.read("sets/use.go");
    has(&uses, "\ts.Insert(2)\n\tb.Add(3)\n");
    has(&uses, "\tt.Insert(4)\n");
    has(&p.read("sets/use_test.go"), "\ts.Insert(1)\n");
    // The other type's method, the comments and the string are as they were.
    has(&sets, "func (b *Bag) Add(x int) { b.n += x }");
    has(&sets, "// Set holds numbers. Add puts one in.");
    has(&uses, "fmt.Println(\"call s.Add(5) to Add more\")");
    // A line where the name stands for two things is left alone and said so, and so is a call
    // on a receiver that could be either type.
    has(&uses, "func both(s *Set, b *Bag) { s.Add(6); b.Add(7) }");
    has(&said, "not changed, 1 line where `Add` stands for more than this (edit by hand):\n  sets/use.go:15: func both(s *Set, b *Bag) { s.Add(6); b.Add(7) }");
    has(&said, "not changed, 1 use of `Add` on a receiver whose type is not known (each may be another type's):\n  sets/use.go:19: x.Add(8)");
    // The index follows the edit: the method answers to its new name.
    has(&p.ask(&["callers", "Set.Insert", "--tests"]), "7 TestAdd: s.Insert(1)");
}

#[test]
fn a_dry_run_changes_nothing() {
    let p = sets("dry");
    let said = p.ask(&["rename", "Set.Add", "Insert", "--dry-run"]);
    has(&said, "would rename Set.Add to Insert: its definition and 4 references in 3 files");
    assert_eq!(p.read("sets/sets.go"), SETS);
    assert_eq!(p.read("sets/use.go"), USE);
}

#[test]
fn a_rename_that_cannot_be_exact_is_refused() {
    let p = sets("refused");
    // Two methods share the name: which one is meant has to be said.
    let said = p.ask(&["rename", "Add", "Insert"]);
    has(&said, "`Add` names 2 functions. Say which one, as Owner.name or path:line. Nothing was changed.");
    // The new name is taken.
    std::fs::write(p.root.join("src/sets/more.go"), "package sets\n\nfunc (s *Set) Insert(x int) {}\n").unwrap();
    let said = p.ask(&["rename", "Set.Add", "Insert"]);
    has(&said, "Set already has a member `Insert` (sets/more.go:3");
    has(&said, "Nothing was changed.");
    // Not a name, and not a function.
    has(&p.ask(&["rename", "Set.Add", "2fast"]), "`2fast` is not a name: nothing was changed.");
    has(&p.ask(&["rename", "Set.items", "things"]), "is not a function or method this index knows");
    assert_eq!(p.read("sets/sets.go"), SETS);
    assert_eq!(p.read("sets/use.go"), USE);
}

#[test]
fn a_function_is_renamed_through_its_imports() {
    let p = Project::new(
        "ts",
        &[
            (
                "lib/money.ts",
                "export function formatPrice(cents: number): string {\n  return String(cents)\n}\n\nexport function other() {\n  return formatPrice(1)\n}\n",
            ),
            ("app/cart.ts", "import { formatPrice } from '../lib/money'\n\nexport const total = (n: number) => formatPrice(n) + ' total'\n"),
            ("app/local.ts", "function formatPrice(x: string) {\n  return x\n}\n\nexport const shown = formatPrice('a')\n"),
        ],
    );
    let said = p.ask(&["rename", "lib/money.ts:1", "priceText"]);
    has(&said, "renamed formatPrice to priceText: its definition and 3 references in 2 files");
    has(&p.read("lib/money.ts"), "export function priceText(cents: number): string {");
    has(&p.read("lib/money.ts"), "  return priceText(1)\n");
    assert_eq!(p.read("app/cart.ts"), "import { priceText } from '../lib/money'\n\nexport const total = (n: number) => priceText(n) + ' total'\n");
    // A function of the same name in a module that does not import it is another function.
    lacks(&p.read("app/local.ts"), "priceText");
}
