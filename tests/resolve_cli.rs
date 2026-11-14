//! End-to-end checks of how references are linked: a small project is written to a temporary
//! directory, indexed, and asked through the command line, the way a harness would.

use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_codefast");

struct Project {
    root: PathBuf,
}

impl Project {
    fn new(name: &str, files: &[(&str, &str)]) -> Project {
        let root = std::env::temp_dir().join(format!("codefast-resolve-{}-{name}", std::process::id()));
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

#[allow(dead_code)]
fn exists(p: &Path) -> bool {
    p.exists()
}

#[test]
fn rust_impl_blocks_do_not_hide_a_reexported_type() {
    let p = Project::new(
        "rust-reexport",
        &[
            ("lib.rs", "pub mod ty;\npub mod user;\n"),
            // The module that re-exports the type also has an `impl` block for it.
            ("ty/mod.rs", "mod context;\npub use self::context::Ctx;\n\nimpl Ctx {\n    pub fn extra(&self) -> u32 {\n        2\n    }\n}\n"),
            ("ty/context.rs", "pub struct Ctx;\n\nimpl Ctx {\n    pub fn run(&self) -> u32 {\n        1\n    }\n}\n"),
            ("user.rs", "use crate::ty::Ctx;\n\npub fn go(c: Ctx) -> u32 {\n    c.run() + c.extra()\n}\n"),
        ],
    );
    let callers = p.ask(&["callers", "Ctx.run"]);
    has(&callers, "all 1 call sites, 1 files");
    has(&callers, "user.rs");
    let refs = p.ask(&["refs", "Ctx"]);
    has(&refs, "user.rs");
    has(&p.ask(&["callees", "go"]), "Ctx.extra ty/mod.rs:5");
}

#[test]
fn javascript_default_exports_and_renamed_exports() {
    let p = Project::new(
        "js-exports",
        &[
            ("lib/named.ts", "function compute(a: number) { return a; }\nexport default compute;\n"),
            ("lib/dflt.ts", "export default function () { return 2; }\n"),
            ("lib/thing.ts", "export default class Thing {\n  run(): number { return 1; }\n}\n"),
            ("lib/ns1.ts", "export function alpha() { return 1; }\nexport function beta() { return 2; }\n"),
            ("lib/ns2.ts", "export function gamma() { return 3; }\nfunction _null() { return null; }\nexport { _null as null };\n"),
            ("lib/anon.js", "module.exports = function (x) {\n  return x * 2;\n};\n"),
            ("main.js", "const twice = require('./lib/anon');\nfunction go() {\n  return twice(2);\n}\n"),
            (
                "app.ts",
                "import Widget from './lib/thing';\nimport dflt from './lib/dflt';\nimport comp from './lib/named';\nimport * as _a from './lib/ns1';\nimport * as _b from './lib/ns2';\nconst z = { ..._a, ..._b };\nexport function run() {\n  const t = new Widget();\n  t.run();\n  dflt();\n  comp(3);\n  z.alpha();\n  z.gamma();\n  z.null();\n}\n",
            ),
        ],
    );
    let callees = p.ask(&["callees", "run", "-p", "app.ts"]);
    for line in [
        "Thing (written Widget) lib/thing.ts:1",
        "Thing.run lib/thing.ts:2",
        "dflt lib/dflt.ts:1",
        "compute (written comp) lib/named.ts:1",
        "alpha lib/ns1.ts:1",
        "gamma lib/ns2.ts:1",
        "_null lib/ns2.ts:2",
    ] {
        has(&callees, line);
    }
    lacks(&callees, "not defined in repo");
    // And the other way round: who calls a function that its importers know under another name.
    has(&p.ask(&["callers", "compute"]), "11 run: comp(3);");
    has(&p.ask(&["callers", "_null"]), "14 run: z.null();");
    has(&p.ask(&["callers", "alpha"]), "12 run: z.alpha();");
    has(&p.ask(&["callers", "lib/dflt.ts:1"]), "10 run: dflt();");
    has(&p.ask(&["callers", "Thing"]), "8 run: const t = new Widget();");
    // A CommonJS module that is a function: calling the binding calls that function.
    has(&p.ask(&["callees", "go"]), "twice lib/anon.js:1");
    lacks(&p.ask(&["callees", "go"]), "not defined in repo");
    has(&p.ask(&["callers", "lib/anon.js:1"]), "3 go: return twice(2);");
}

#[test]
fn typescript_constant_objects_and_aliases() {
    let p = Project::new(
        "ts-objects",
        &[
            (
                "registry.ts",
                "export interface IRegistry {\n  as(id: string): number;\n}\nclass RegistryImpl implements IRegistry {\n  as(id: string): number {\n    return 1;\n  }\n}\nexport const Registry: IRegistry = new RegistryImpl();\n",
            ),
            (
                "shapes.ts",
                "export function makeShape(n: number) {\n  return n;\n}\nexport class Shape {\n  static create = makeShape;\n  grow(by: number) {\n    return by;\n  }\n  enlarge = this.grow;\n  twice() {\n    return this.enlarge(2);\n  }\n}\nexport const shapeOf = Shape.create;\n",
            ),
            (
                "use.ts",
                "import { Registry } from './registry';\nimport { Shape, shapeOf } from './shapes';\nexport function f() {\n  return Registry.as('x') + Shape.create(1) + shapeOf(2);\n}\n",
            ),
        ],
    );
    // A capitalised constant is an object, not a type: the call lands in its declared type.
    let callers = p.ask(&["callers", "IRegistry.as"]);
    has(&callers, "all 1 call sites, 1 files");
    has(&callers, "use.ts");
    has(&p.ask(&["callers", "RegistryImpl.as"]), "use.ts");
    // A static field and a variable initialised with a function stand for that function.
    let callees = p.ask(&["callees", "f"]);
    has(&callees, "as registry.ts:2");
    has(&callees, "makeShape (written create) shapes.ts:1");
    has(&callees, "makeShape (written shapeOf) shapes.ts:1");
    lacks(&callees, "not defined in repo");
    let callers = p.ask(&["callers", "makeShape"]);
    has(&callers, "dispatched via Shape.create (1 call sites):");
    has(&callers, "4 f: return Registry.as('x') + Shape.create(1) + shapeOf(2);");
    // A field initialised with a method: a call of the field reaches the method.
    has(&p.ask(&["callees", "Shape.twice"]), "Shape.grow shapes.ts:6");
}

#[test]
fn go_method_values_are_value_uses() {
    let p = Project::new(
        "go-values",
        &[(
            "main.go",
            "package main\n\ntype Controller struct{}\n\nfunc (c *Controller) worker() {\n\tc.step()\n}\n\nfunc (c *Controller) step() {}\n\nfunc until(f func()) {\n\tf()\n}\n\nfunc (c *Controller) Run() {\n\tuntil(c.worker)\n}\n",
        )],
    );
    let callers = p.ask(&["callers", "Controller.worker"]);
    has(&callers, "1 uses as a value");
    has(&callers, "16 Run: until(c.worker)");
    has(&p.ask(&["callees", "Controller.Run"]), "worker main.go:5 [passed on as a value]");
    let path = p.ask(&["path", "Controller.Run", "Controller.step"]);
    has(&path, "Run → worker → step");
}

#[test]
fn an_index_built_for_another_repository_is_never_served() {
    // One index directory, named by the environment, used from two repositories in turn.
    let a = Project::new("shared-a", &[("a.c", "int only_in_a(void)\n{\n    return 1;\n}\n")]);
    let b = Project::new("shared-b", &[("b.c", "int only_in_b(void)\n{\n    return 2;\n}\n")]);
    let ask = |p: &Project, name: &str| {
        let out = Command::new(BIN)
            .arg("--root")
            .arg(p.root.join("src"))
            .args(["find", name])
            .env("CODEFAST_INDEX_DIR", a.root.join("index"))
            .output()
            .expect("run codefast");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    has(&ask(&a, "only_in_a"), "a.c:1");
    has(&ask(&b, "only_in_b"), "b.c:1");
    lacks(&ask(&b, "only_in_a"), "a.c:1");
    has(&ask(&a, "only_in_a"), "a.c:1");
    lacks(&ask(&a, "only_in_b"), "b.c:1");
}

#[test]
fn c_static_calls_and_pasting_macros() {
    let p = Project::new(
        "c-macros",
        &[
            (
                "ops.h",
                "struct regs;\nstruct pmu_ops {\n\tint (*handle_irq)(struct regs *r);\n\tint (*vcpu_run)(int v);\n};\nextern struct pmu_ops pmu;\n#define DEFINE_STATIC_CALL_NULL(name, fn) int name\n#define static_call(name) (name##_tramp)\n#define static_call_update(name, fn) do { } while (0)\n#define ops_call(func) static_call(ops_##func)\n#define OPS_KEY(func) DEFINE_STATIC_CALL_NULL(ops_##func, *(((struct pmu_ops *)0)->func));\n",
            ),
            (
                "core.c",
                "#include \"ops.h\"\n\nDEFINE_STATIC_CALL_NULL(pmu_handle_irq, *pmu.handle_irq);\n\nstatic int nmi_handler(struct regs *r)\n{\n\treturn static_call(pmu_handle_irq)(r);\n}\n\nstatic void setup(void)\n{\n\tstatic_call_update(pmu_handle_irq, pmu.handle_irq);\n}\n\nint enter_guest(int v)\n{\n\treturn ops_call(vcpu_run)(v);\n}\n",
            ),
            (
                "intel.c",
                "#include \"ops.h\"\n\nstatic int intel_handle_irq(struct regs *r)\n{\n\treturn 1;\n}\n\nint vmx_vcpu_run(int v)\n{\n\treturn v;\n}\n\n#define vt_op(name) vmx_##name\n\nstruct pmu_ops intel_ops __initdata = {\n\t.handle_irq = intel_handle_irq,\n\t.vcpu_run = vt_op(vcpu_run),\n};\n",
            ),
        ],
    );
    // A variable declared with a trailing attribute macro keeps its own name.
    has(&p.ask(&["find", "intel_ops"]), "intel.c:15");
    // A function bound into a slot is reached through the static call tied to that slot.
    let callers = p.ask(&["callers", "intel_handle_irq"]);
    has(&callers, "dispatched via pmu_ops.handle_irq");
    has(&callers, "7 nmi_handler: return static_call(pmu_handle_irq)(r);");
    // A table entry written through a name-pasting macro, called through a wrapper macro.
    let refs = p.ask(&["refs", "pmu_ops.vcpu_run"]);
    has(&refs, "vmx_vcpu_run intel.c:8  (written vt_op(vcpu_run) at intel.c:17)");
    has(&refs, "17 enter_guest: return ops_call(vcpu_run)(v);");
    let callers = p.ask(&["callers", "vmx_vcpu_run"]);
    has(&callers, "pmu_ops.vcpu_run in intel_ops");
    has(&callers, "17 enter_guest: return ops_call(vcpu_run)(v);");
}

#[test]
fn references_to_a_member_that_is_gone_are_listed() {
    // The definition was renamed and the call sites were not: what is left to fix.
    let p = Project::new(
        "go-dangling",
        &[
            (
                "set.go",
                "package main\n\ntype Set struct{ items []int }\n\nfunc (s *Set) AddEntry(v int) {\n\ts.items = append(s.items, v)\n}\n\ntype Other struct{}\n\nfunc (o *Other) Add(v int) {}\n",
            ),
            ("use.go", "package main\n\nfunc fill(s *Set, o *Other) {\n\ts.Add(1)\n\to.Add(2)\n\ts.Add(3)\n}\n"),
        ],
    );
    let out = p.ask(&["callers", "Set.Add"]);
    has(&out, "has no member `Add`. References that expect one (2 in 1 files)");
    has(&out, "4 fill: s.Add(1)");
    has(&out, "6 fill: s.Add(3)");
    lacks(&out, "o.Add(2)");
}

/// Two types have a method `Add`. Which one a call means is decided by the receiver's type,
/// also where that type is written in another function's signature.
const GO_STORE: &str = r#"package store

import (
	"example.com/ext"
	"example.com/ext/progress"
)

type Set struct{}

func (s *Set) Add(x int) {}

type Bag struct{}

func (b *Bag) Add(x int) {}

type Item struct{}

func (i Item) Touch() {}

type Other struct{}

func (o Other) Touch() {}

// Sets is a map of sets by name.
type Sets map[string]*Set

// Progress gets its Add from a type of another package.
type Progress struct {
	*progress.Manual
}

// Reader is answered by whatever implements the interface it embeds.
type Reader interface {
	ext.Adder
}

func load() (int, *Set, error)        { return 0, nil, nil }
func named() (n int, bag *Bag, e error) { return 0, nil, nil }
func each() <-chan Item               { return nil }
func byName() map[Other]Item          { return nil }
func all() Sets                       { return nil }
func start() *Progress                { return nil }
func logger() ext.Logger              { return nil }
func reader() Reader                  { return nil }
"#;

const GO_USE: &str = r#"package store

func second() {
	_, s, err := load()
	_ = err
	s.Add(1)
}

func secondNamed() {
	_, b, _ := named()
	b.Add(2)
}

func received() {
	for it := range each() {
		it.Touch()
	}
}

func keys() {
	for k := range byName() {
		k.Touch()
	}
}

func elements() {
	all()["a"].Add(3)
}

func promoted() {
	start().Add(4)
}

func foreign() {
	logger().Add(5)
}

func dynamic() {
	reader().Add(6)
}
"#;

#[test]
fn go_receivers_typed_by_another_functions_signature() {
    let p = Project::new("go-results", &[("store/store.go", GO_STORE), ("store/use.go", GO_USE)]);
    // The second result of `load`, and the elements of the named map `all` returns. The calls
    // on a type of another package (through a function's result, or promoted from an embedded
    // field) are nobody's here. The call on an interface that embeds a foreign one may reach
    // either `Add`: it stays a match by name.
    let set = p.ask(&["callers", "Set.Add"]);
    has(&set, "2 call sites on a receiver of known type, 1 files, 1 more by name");
    has(&set, "6 second: s.Add(1)");
    has(&set, "27 elements: all()[\"a\"].Add(3)");
    has(&set, "39 dynamic: reader().Add(6)");
    for other in ["b.Add(2)", "start().Add(4)", "logger().Add(5)"] {
        lacks(&set, other);
    }
    // The second result where the results are named.
    let bag = p.ask(&["callers", "Bag.Add"]);
    has(&bag, "11 secondNamed: b.Add(2)");
    lacks(&bag, "s.Add(1)");
    // One name ranging over a call is an element when the call returns a channel, and a key
    // when it returns a map.
    let touch = p.ask(&["callers", "Item.Touch"]);
    has(&touch, "all 1 call sites, 1 files");
    has(&touch, "16 received: it.Touch()");
    let other = p.ask(&["callers", "Other.Touch"]);
    has(&other, "all 1 call sites, 1 files");
    has(&other, "22 keys: k.Touch()");
}

#[test]
fn go_result_lists_are_read_as_written() {
    // Through the command line: each of these is a function whose later result types a call.
    let lib = "package lib\n\ntype A struct{}\n\nfunc (a A) Hit() {}\n\ntype B struct{}\n\nfunc (b B) Hit() {}\n\n\
               func plain() (B, A)                          { return B{}, A{} }\n\
               func grouped() (x, y B, z A)                 { return B{}, B{}, A{} }\n\
               func nested(f func(int) (B, error)) (B, []A) { return B{}, nil }\n\
               func generic[T any](v T) (T, map[string]*A)  { return v, nil }\n\
               type R struct{}\n\nfunc (r *R) method() (B, chan A) { return B{}, nil }\n";
    let uses = "package lib\n\nfunc one() {\n\t_, a := plain()\n\ta.Hit()\n}\n\nfunc two() {\n\t_, _, a := grouped()\n\ta.Hit()\n}\n\n\
                func three() {\n\t_, as := nested(nil)\n\tas[0].Hit()\n}\n\nfunc four() {\n\t_, m := generic(1)\n\tm[\"k\"].Hit()\n}\n\n\
                func five(r *R) {\n\t_, ch := r.method()\n\tx := <-ch\n\tx.Hit()\n}\n";
    let p = Project::new("go-result-lists", &[("lib/lib.go", lib), ("lib/uses.go", uses)]);
    let a = p.ask(&["callers", "A.Hit"]);
    for site in ["5 one: a.Hit()", "10 two: a.Hit()", "15 three: as[0].Hit()", "20 four: m[\"k\"].Hit()"] {
        has(&a, site);
    }
    lacks(&p.ask(&["callers", "B.Hit"]), "Hit()\n");
}

#[test]
fn go_table_tests_type_their_cases() {
    // The rows of a table-driven test are anonymous structs. What a field of a row is has to
    // be read where the table is declared.
    let src = "package tbl\n\ntype Set struct{}\n\nfunc (s *Set) Add(x int) {}\n\ntype Bag struct{}\n\nfunc (b *Bag) Add(x int) {}\n\ntype Case struct{ S *Set }\n\n\
               func run() {\n\ttests := []struct {\n\t\tname string\n\t\tset  *Set\n\t\tbags []Bag\n\t\tdeep struct{ inner Case }\n\t}{}\n\
               \tfor _, tt := range tests {\n\t\ttt.set.Add(1)\n\t\ttt.bags[0].Add(2)\n\t\ttt.deep.inner.S.Add(3)\n\t}\n\
               \tbyName := map[string]struct{ set *Set }{}\n\tfor _, c := range byName {\n\t\tc.set.Add(4)\n\t}\n\
               \tvar one struct{ bag Bag }\n\tone.bag.Add(5)\n\
               \tfor _, row := range []struct{ b *Bag }{{}} {\n\t\tfunc() { row.b.Add(6) }()\n\t}\n}\n";
    let p = Project::new("go-tables", &[("tbl/tbl.go", src)]);
    let set = p.ask(&["callers", "Set.Add"]);
    has(&set, "all 3 call sites, 1 files");
    for site in ["21 run: tt.set.Add(1)", "23 run: tt.deep.inner.S.Add(3)", "27 run: c.set.Add(4)"] {
        has(&set, site);
    }
    let bag = p.ask(&["callers", "Bag.Add"]);
    has(&bag, "all 3 call sites, 1 files");
    for site in ["22 run: tt.bags[0].Add(2)", "30 run: one.bag.Add(5)", "row.b.Add(6)"] {
        has(&bag, site);
    }
}

#[test]
fn context_names_the_types_of_a_signature_and_the_tests_two_calls_away() {
    // A helper deep inside a package is not called by a test. The test calls what the package
    // offers, and that reaches the helper through one function or two.
    let code = "package codec\n\ntype span struct {\n\tstart int\n\tend   int\n}\n\n// shift moves a span by an offset.\nfunc shift(s span, by int) span {\n\treturn span{s.start + by, s.end + by}\n}\n\n\
                func locate(s span) span {\n\treturn shift(s, 4)\n}\n\nfunc Decode(s span) span {\n\treturn locate(s)\n}\n";
    let test = "package codec\n\nimport \"testing\"\n\nfunc TestDecode(t *testing.T) {\n\tif Decode(span{1, 2}).start != 5 {\n\t\tt.Fatal(\"wrong\")\n\t}\n}\n";
    let p = Project::new("go-context", &[("codec/segment.go", code), ("codec/segment_test.go", test)]);
    let shift = p.ask(&["context", "shift"]);
    // What goes in and what comes out, as the repository defines it, once.
    has(&shift, "its signature names:\ncodec/segment.go:3-6 span\n3\ttype span struct {");
    assert_eq!(shift.matches("codec/segment.go:3-6 span").count(), 1, "{shift}");
    has(&shift, "called from (1 site):\n  codec/segment.go:14 locate: return shift(s, 4)");
    has(&shift, "tests that reach it (1):\n  codec/segment_test.go:6 TestDecode  (through locate and Decode)");
    has(&shift, "run: go test ./codec/ -run '^(TestDecode)$'");
    // One call away is said as before, and a test that calls it is not said to go through anything.
    has(&p.ask(&["context", "locate"]), "codec/segment_test.go:6 TestDecode  (through Decode)");
    has(&p.ask(&["context", "Decode"]), "tests that reach it (1):\n  codec/segment_test.go:6 TestDecode\n");
}

#[test]
fn a_member_asked_for_on_the_wrong_owner_is_found_where_it_is() {
    // An agent that has to write `Branch.marshal` asks for the method next door to copy from,
    // and guesses the name of the type. "No matches" sends it back to grep.
    let src = "package config\n\ntype Branch struct{ Name string }\n\nfunc (b *Branch) marshal() string {\n\treturn b.Name\n}\n\n\
               type RemoteConfig struct{ Name string }\n\nfunc (c *RemoteConfig) marshal() string {\n\treturn \"remote \" + c.Name\n}\n\n\
               func (c *RemoteConfig) Validate() error {\n\treturn nil\n}\n\ntype URL struct{ Name string }\n\nfunc (u *URL) marshal() string {\n\treturn u.Name\n}\n";
    let p = Project::new("go-wrong-owner", &[("config/config.go", src)]);
    // The type is called something a little longer: the method, with its body.
    let remote = p.ask(&["search", "Remote.marshal"]);
    has(&remote, "no `Remote.marshal`; the one `marshal` on a type of a similar name is RemoteConfig.marshal:");
    has(&remote, "return \"remote \" + c.Name");
    // No type of a similar name: every type that has the member, with its place.
    let other = p.ask(&["search", "Submodule.marshal"]);
    has(&other, "no `Submodule.marshal`; `marshal` is defined on 3 others:");
    for at in ["config/config.go:5 func (b *Branch) marshal() string", "config/config.go:11 func (c *RemoteConfig) marshal() string", "config/config.go:21 func (u *URL) marshal() string"] {
        has(&other, at);
    }
    // The right owner, and the name in another case.
    let cased = p.ask(&["search", "RemoteConfig.validate"]);
    has(&cased, "no `RemoteConfig.validate`; the one member of RemoteConfig with a similar name is RemoteConfig.Validate:");
    // What exists is answered as before.
    let exact = p.ask(&["search", "RemoteConfig.marshal"]);
    has(&exact, "config/config.go:11-13 RemoteConfig.marshal");
    lacks(&exact, "no `");
}
