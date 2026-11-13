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
